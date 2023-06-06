use std::{env, fs, path::PathBuf, time::UNIX_EPOCH};

use anyhow::{anyhow, Context, Error};
use chrono::{
    naive::{NaiveDate, NaiveTime},
    DateTime, Datelike, Utc,
};
use influxdb::{Client, InfluxDbWriteable, ReadQuery, WriteQuery};
use rayon::prelude::*;
use serde::Deserialize;
use walkdir::{DirEntry, WalkDir};
use yaml_front_matter::YamlFrontMatter;

const DB_HOST_VAR_HANDLE: &str = "DB_HOST";
const DB_NAME_VAR_HANDLE: &str = "DB_NAME";
const DB_PORT_VAR_HANDLE: &str = "DB_PORT";
const NOTES_DIR_VAR_HANDLE: &str = "NOTES_DIR";
const VAULT_PATH_VAR_HANDLE: &str = "VAULT_PATH";
const NOTE_FILE_EXTENSION: &str = ".md";
const DATE_FORMAT: &str = "%Y-%m-%d";

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
    frontmatter: Frontmatter,
    date: NaiveDate,
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

fn get_env_var(handle: &str) -> Result<String, Error> {
    env::var(handle).context(format!("Could not get env var {handle}"))
}

fn build_vault_path(path: &str, dir: &str) -> PathBuf {
    let mut notes_path: PathBuf = PathBuf::from(path);

    notes_path.push(dir);

    println!("Vault path: {:?}", notes_path.as_os_str());

    notes_path
}

async fn get_date_from_query(
    client: &Client,
    read_query: ReadQuery,
) -> Result<DateTime<Utc>, Error> {
    let mut db_result = client.json_query(read_query).await?;

    Ok(db_result
        .deserialize_next::<DbEntry>()?
        .series
        .first()
        .context("No elements in series")?
        .values
        .first()
        .context("No values in first element of series")?
        .time)
}

async fn get_starting_date(client: &Client, config: &Config) -> DateTime<Utc> {
    println!("Reading latest entry from InfluxDB...");

    let read_query: ReadQuery = ReadQuery::new(format!(
        "SELECT * FROM {} ORDER BY time DESC LIMIT 1",
        &config.db_name
    ));

    (get_date_from_query(client, read_query).await).map_or_else(
        |e| {
            println!("Could not get date from latest entry: {e}");
            UNIX_EPOCH.into()
        },
        |val| val,
    )
}

fn note_from_path(path: &str, date: NaiveDate) -> Option<Note> {
    let file_contents: String = fs::read_to_string(path).ok()?;

    let frontmatter: Frontmatter = YamlFrontMatter::parse::<Frontmatter>(file_contents.as_str())
        .ok()?
        .metadata;

    Some(Note { frontmatter, date })
}

fn parse_file_to_note(entry: &DirEntry, starting_date: NaiveDate) -> Option<Note> {
    let entry_path = entry.path().to_str()?;

    let file_date: NaiveDate = NaiveDate::parse_from_str(entry_path, DATE_FORMAT).ok()?;

    let yesterday = Utc::now().date_naive().pred_opt()?;

    if file_date > starting_date && file_date <= yesterday {
        note_from_path(entry_path, file_date)
    } else {
        None
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map_or(false, |s| s.starts_with('.'))
}

fn get_sorted_notes_from_dir(path: PathBuf, starting_date: NaiveDate) -> Vec<Note> {
    println!("Getting notes from dir {:?}", path.as_os_str());

    let mut notes = WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| {
            !is_hidden(e)
                && e.file_name()
                    .to_str()
                    .map_or(false, |s| s.ends_with(NOTE_FILE_EXTENSION))
        })
        .par_bridge()
        .filter_map(Result::ok)
        .filter_map(|e| parse_file_to_note(&e, starting_date))
        .collect::<Vec<Note>>();

    notes.sort_by(|a, b| a.date.cmp(&b.date));

    notes
}

async fn push_notes_data(config: Config, client: Client) -> Result<(), Error> {
    let starting_date: NaiveDate = get_starting_date(&client, &config).await.date_naive();

    println!("Using {starting_date} as starting point...");

    println!("Adding notes...");

    let notes_path: PathBuf =
        build_vault_path(config.vault_path.as_str(), config.notes_dir.as_str());

    let notes: Vec<Note> = get_sorted_notes_from_dir(notes_path, starting_date);

    if notes.is_empty() {
        println!("No notes were found");
        return Ok(());
    }

    let db_name = config.db_name.as_str();

    let inserts: Vec<WriteQuery> = notes
        .into_iter()
        .flat_map(|note| {
            let note_time = note.date;
            let weekday = note.date.weekday();

            note.frontmatter
                .tags
                .into_iter()
                .enumerate()
                .filter(|(_, tag)| tag.contains('#'))
                .map(move |(index, tag)| {
                    let entry: WriteQuery = DbEntry {
                        time: note_time
                            .and_time(
                                NaiveTime::from_num_seconds_from_midnight_opt(
                                    0,
                                    std::convert::TryInto::try_into(index).unwrap(),
                                )
                                .unwrap(),
                            )
                            .and_utc(),
                        weekday: weekday.to_string(),
                        frontmatter_tag: tag,
                        value: 1,
                    }
                    .into_query(db_name);

                    entry
                })
        })
        .collect();

    if inserts.is_empty() {
        return Err(anyhow!(
            "Notes were found, but no insert queries were generated"
        ));
    }

    let _res = client.query(inserts).await?;

    println!("Finished!");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("Configuring...");

    let config = Config {
        db_host: get_env_var(DB_HOST_VAR_HANDLE)?,
        db_name: get_env_var(DB_NAME_VAR_HANDLE)?,
        db_port: get_env_var(DB_PORT_VAR_HANDLE)?,
        notes_dir: get_env_var(NOTES_DIR_VAR_HANDLE)?,
        vault_path: get_env_var(VAULT_PATH_VAR_HANDLE)?,
    };

    println!("Configuration loaded!");

    let client: Client = Client::new(
        format!("http://{}:{}", &config.db_host, &config.db_port),
        &config.db_name,
    );

    println!("Configuration done!");

    push_notes_data(config, client).await
}
