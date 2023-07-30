# Obsidian to Influx
Tool to parse the frontmatter of Obsidian MD daily notes and push tags to InfluxDB.

The intention was to be able to create a Grafana dashboard with this data, however it's been WIP for some time.

## Usage

When running for the first time, tags from all notes up until the previous day are pushed to Influx, with a timestamp based on the note's date.

On subsequent runs, tags are only pushed for notes starting the day after the latest timestamp on Influx. Some tags can be missed due to this.

### Docker

This is a one off run rather than a continuous program.
Therefore, setting up a cron job to start the container (e.g. once a day) is the best way to use this.

#### Env Variables

| Variable | Description |
|----------|-------------|
| `DB_HOST` | InfluxDB host |
| `DB_PORT` | InfluxDB port |
| `DB_NAME` | InfluxDB database name |
| `VAULT_PATH` | Path to Obsidian vault |
| `NOTES_DIR` | Directory of daily notes to be parsed |

### Without Docker
Compile and run after setting env variables
