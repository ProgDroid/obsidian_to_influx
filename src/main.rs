use chrono::naive::{NaiveDate, NaiveTime};
use chrono::{Date, DateTime, Datelike, Utc};
use influxdb::integrations::serde_integration::Return;
use influxdb::{Client, InfluxDbWriteable, ReadQuery};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use yaml_front_matter::YamlFrontMatter;

const DB_HOST_VAR_HANDLE: &str = "DB_HOST";
const DB_NAME_VAR_HANDLE: &str = "DB_NAME";
const DB_PORT_VAR_HANDLE: &str = "DB_PORT";
const NOTES_DIR_VAR_HANDLE: &str = "NOTES_DIR";
const VAULT_PATH_VAR_HANDLE: &str = "VAULT_PATH";

struct Config {
    db_host: String,
    db_name: String,
    db_port: String,
    notes_dir: String,
    vault_path: String,
}

#[derive(Deserialize, Debug)]
struct Frontmatter {
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug)]
struct Note {
    path: PathBuf,
    frontmatter: Frontmatter,
    date: Date<Utc>,
}

#[derive(Debug, Deserialize, InfluxDbWriteable)]
struct DbEntry {
    time: DateTime<Utc>,
    #[influxdb(tag)]
    weekday: String,
    #[influxdb(tag)]
    frontmatter_tag: String,
    value: u8,
}

fn get_env_var(handle: &str) -> String {
    println!("Getting env var {}", handle);

    match env::var(handle) {
        Ok(val) => val,
        Err(e) => {
            println!("Could not get env var {}: {}", handle, e);
            std::process::exit(exitcode::CONFIG)
        }
    }
}

fn build_vault_path(path: &str, dir: &str) -> PathBuf {
    let mut notes_path: PathBuf = PathBuf::from(path);

    notes_path.push(dir);

    println!("Vault path: {:?}", notes_path.as_os_str());

    notes_path
}

fn get_starting_date_from_query_result(db_entry: Return<DbEntry>) -> DateTime<Utc> {
    match db_entry.series.iter().next() {
        None => UNIX_EPOCH.into(),
        Some(series) => match series.values.iter().next() {
            Some(db_entry) => db_entry.time,
            None => UNIX_EPOCH.into(),
        },
    }
}

async fn get_starting_date(client: &Client, config: &Config) -> DateTime<Utc> {
    println!("Reading latest entry from InfluxDB...");

    let read_query: ReadQuery = ReadQuery::new(format!(
        "SELECT * FROM {} ORDER BY time DESC LIMIT 1",
        &config.db_name
    ));

    match client
        .json_query(read_query)
        .await
        .and_then(|mut db_result| db_result.deserialize_next::<DbEntry>())
    {
        Ok(read_result) => get_starting_date_from_query_result(read_result),
        Err(_) => UNIX_EPOCH.into(),
    }
}

fn date_from_file_name(file_name: String) -> Result<Date<Utc>, &'static str> {
    let naive_date: NaiveDate = match NaiveDate::parse_from_str(&file_name, "%Y-%m-%d") {
        Ok(value) => value,
        Err(_) => {
            eprintln!("{}", &file_name);
            return Err("Could not parse date from file name");
        }
    };

    Ok(Date::<Utc>::from_utc(naive_date, Utc))
}

fn note_from_path(path: &PathBuf, date: Date<Utc>) -> Result<Note, &'static str> {
    let file_contents: String = match fs::read_to_string(path.as_path()) {
        Ok(value) => value,
        Err(_) => return Err("Could not read file contents"),
    };

    let frontmatter: Frontmatter =
        match YamlFrontMatter::parse::<Frontmatter>(file_contents.as_str()) {
            Ok(value) => value.metadata,
            Err(_) => return Err("Could not get frontmatter from note"),
        };

    Ok(Note {
        path: path.to_path_buf(),
        frontmatter: frontmatter,
        date: date,
    })
}

fn get_notes_from_dir(path: PathBuf, starting_date: Date<Utc>) -> Vec<Note> {
    println!("Getting notes from dir {:?}", path.as_os_str());

    let entries = match fs::read_dir(path) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("Could not read directory: {}", e);
            return Vec::new();
        }
    };

    let mut notes: Vec<Note> = Vec::new();

    for entry in entries {
        let entry_path: PathBuf = entry.unwrap().path();

        if entry_path.as_path().is_dir() {
            notes.append(&mut get_notes_from_dir(entry_path, starting_date));
            continue;
        }

        let file_name: String = match entry_path.file_stem() {
            Some(value) => match value.to_os_string().into_string() {
                Ok(value) => value,
                Err(e) => {
                    eprintln!("Could not get file name: {:?}", e);
                    continue;
                }
            },
            None => continue,
        };

        let file_date: Date<Utc> = match date_from_file_name(file_name) {
            Ok(date) => date,
            Err(e) => {
                eprintln!("Could not get date from file name: {}", e);
                continue;
            }
        };

        let yesterday: Date<Utc> = match Utc::today().pred_opt() {
            Some(value) => value,
            None => {
                eprintln!("Could not get yesterday's date!");
                continue;
            }
        };

        if file_date > starting_date && file_date <= yesterday {
            match note_from_path(&entry_path, file_date) {
                Ok(note) => notes.push(note),
                Err(e) => eprintln!(
                    "Error getting note from path {}: {}",
                    entry_path.display(),
                    e
                ),
            };
        }
    }

    notes
}

async fn add_notes_data(config: Config, client: Client, starting_date: Date<Utc>) -> i32 {
    println!("Adding notes...");

    let notes_path: PathBuf =
        build_vault_path(config.vault_path.as_str(), config.notes_dir.as_str());

    let mut notes: Vec<Note> = get_notes_from_dir(notes_path, starting_date);

    notes.sort_by(|a, b| a.date.cmp(&b.date));

    let notes: Vec<Note> = notes;

    for note in notes {
        let mut count: u32 = 0u32;

        for tag in &note.frontmatter.tags {
            if tag.contains("#") {
                let entry: DbEntry = DbEntry {
                    time: note
                        .date
                        .and_time(NaiveTime::from_num_seconds_from_midnight(0, count))
                        .unwrap(),
                    weekday: note.date.weekday().to_string(),
                    frontmatter_tag: tag.to_string(),
                    value: 1,
                };

                count += 1;

                match client.query(entry.into_query(&config.db_name)).await {
                    Ok(_) => println!(
                        "Note {:?}, tag {} pushed into InfluxDB",
                        note.path.file_stem().unwrap(),
                        tag
                    ),
                    Err(e) => eprintln!("Could not push to InfluxDB: {}", e),
                };
            }
        }
    }

    println!("Finished...");

    exitcode::OK
}

#[tokio::main]
async fn main() {
    let config = Config {
        db_host: get_env_var(DB_HOST_VAR_HANDLE),
        db_name: get_env_var(DB_NAME_VAR_HANDLE),
        db_port: get_env_var(DB_PORT_VAR_HANDLE),
        notes_dir: get_env_var(NOTES_DIR_VAR_HANDLE),
        vault_path: get_env_var(VAULT_PATH_VAR_HANDLE),
    };

    let client: Client = Client::new(
        format!("http://{}:{}", &config.db_host, &config.db_port),
        &config.db_name,
    );

    let starting_date: Date<Utc> = get_starting_date(&client, &config).await.date();

    println!("Using {} as starting point", starting_date);

    std::process::exit(add_notes_data(config, client, starting_date).await);
}
