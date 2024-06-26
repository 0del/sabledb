use redis::Value;
mod sdb_cli_options;
use bytes::BytesMut;
use clap::Parser;
use sdb_cli_options::Options;
use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

#[allow(unused_imports)]
use redis::Commands;
use rustyline::{Config, Editor};

lazy_static::lazy_static! {
    static ref INDENT_NEEDED: AtomicBool = AtomicBool::new(true);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::fmt()
        .with_thread_names(true)
        .with_thread_ids(true)
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Options::parse();

    let (connection_string, prompt) = if args.tls {
        (
            format!("rediss://{}:{}/#insecure", args.host, args.port),
            format!("{}:{} (TLS) $ ", args.host, args.port),
        )
    } else {
        (
            format!("redis://{}:{}", args.host, args.port),
            format!("{}:{} $ ", args.host, args.port),
        )
    };

    tracing::debug!("Connecting to {}..", connection_string);
    let client = redis::Client::open(connection_string.as_str())?;
    let mut conn = client.get_connection()?;

    if let Some(input_file) = args.file {
        tracing::debug!("Loading file: {input_file}");
        // read the file's content. Each line in the file is considered as a command
        let file_content = fs::read_to_string(input_file)?;
        let commands: Vec<&str> = file_content.split('\n').collect();
        let commands: Vec<String> = commands
            .iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().into())
            .collect();
        tracing::debug!("Running commands: {:#?}", commands);
        batch_execute(&mut conn, commands)?;
    } else if !args.parameters.is_empty() {
        // construct a command from the parameters and execute it
        let command = vec![args.parameters.join(" ")];
        batch_execute(&mut conn, command)?;
    } else {
        interactive_loop(prompt, conn)?;
    }
    Ok(())
}

/// Run `sdb` in an interactive mode
fn interactive_loop(
    prompt: String,
    mut conn: redis::Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::builder().auto_add_history(true).build();
    let history =
        rustyline::sqlite_history::SQLiteHistory::open(config, ".sdb-cli-history.sqlite3")?;
    let mut rl: Editor<(), _> = Editor::with_history(config, history)?;
    loop {
        let Ok(line) = rl.readline(prompt.as_str()) else {
            break;
        };
        batch_execute(&mut conn, vec![line])?;
    }
    Ok(())
}

/// Execute list of redis commands
fn batch_execute(
    conn: &mut redis::Connection,
    commands: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for command in &commands {
        let command = command.trim();
        let mut line = BytesMut::from(command);
        line.extend_from_slice(b"\r\n");
        conn.send_packed_command(line.as_ref())?;
        match conn.recv_response() {
            Err(e) => tracing::error!("{:#?}", e),
            Ok(response) => {
                INDENT_NEEDED.store(true, Ordering::Relaxed);
                print_response_pretty(&response, 0, None);
            }
        }
    }
    Ok(())
}

fn print_response_pretty(value: &Value, indent: usize, seq: Option<usize>) {
    match value {
        Value::Nil => {
            print_sequence(seq);
            println_value("(nil)");
        }
        Value::Int(val) => {
            print_sequence(seq);
            println_value(format!("(integer) {}", val));
        }
        Value::Data(ref val) => {
            let s = String::from_utf8_lossy(val);
            print_sequence(seq);
            println_value(format!(r#""{}""#, s));
        }
        Value::Bulk(ref values) => {
            if values.is_empty() {
                print_indent(indent);
                print_sequence(seq);
                println_value("(empty array)");
            } else {
                print_sequence(seq);
                let mut new_seq = 1usize;
                for val in values.iter() {
                    print_indent(indent);
                    print_response_pretty(val, indent + 4, Some(new_seq));
                    new_seq += 1;
                }
            }
        }
        Value::Okay => {
            print_sequence(seq);
            println_value("OK");
        }
        Value::Status(ref s) => {
            print_sequence(seq);
            println_value(format!(r#""{}""#, s));
        }
    }
}

fn print_indent(indent: usize) {
    if !INDENT_NEEDED.load(Ordering::Relaxed) {
        return;
    }
    let indent = (0..indent).map(|_| " ").collect::<String>();
    print!("{}", indent);
    INDENT_NEEDED.store(false, Ordering::Relaxed);
}

fn print_sequence(seq: Option<usize>) {
    if let Some(seq) = seq {
        print!("{: >2}) ", seq);
    }
}

fn println_value(val: impl std::fmt::Display) {
    println!("{}", val);
    INDENT_NEEDED.store(true, Ordering::Relaxed);
}
