//! `lakelet-load` — the client that puts telemetry into a running store.
//!
//! A store is only judged by what a *client* can make it do, and this is that client: it speaks the
//! same protocol the daemon reads, from a separate process, which is the shape every later step
//! needs — `4.15` profiles a run of it, `4.17` soaks through it, and the acceptance drill of the
//! track drives it from a shell.
//!
//! It is deterministic: `--seed` decides the series names, the timestamps and the values, so two runs
//! with the same arguments send the same bytes. That is what makes it usable as evidence.

use std::process::ExitCode;

use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::Frame;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Everything this program can tell a user about itself.
const USAGE: &str = "\
usage: lakelet-load [--target <addr>] [--samples <n>] [--batch <n>] [--seed <n>] [--series <n>] \
[--help] [--version]";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lakelet-load: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), lakelet::Error> {
    let options = Options::from_args(std::env::args().skip(1))?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }
    if options.version {
        println!("lakelet-load {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let samples = simulate(&options);
    let mut connection =
        TcpStream::connect(&options.target)
            .await
            .map_err(|error| lakelet::Error::Store {
                source: Box::new(error),
            })?;
    let mut sent = 0u64;
    for chunk in samples.chunks(options.batch.max(1)) {
        let payload = batch::encode(chunk);
        let frame = Frame::new(payload);
        connection
            .write_all(&frame.encode())
            .await
            .map_err(|error| lakelet::Error::Store {
                source: Box::new(error),
            })?;
        sent += chunk.len() as u64;
    }
    connection
        .flush()
        .await
        .map_err(|error| lakelet::Error::Store {
            source: Box::new(error),
        })?;
    println!(
        "sent={sent} batches={}",
        samples.len().div_ceil(options.batch.max(1))
    );
    Ok(())
}

/// What one run was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    /// Address of the daemon's ingest port.
    target: String,
    /// How many samples to send.
    samples: usize,
    /// How many samples per frame.
    batch: usize,
    /// Seed of the workload.
    seed: u64,
    /// How many distinct series the workload spreads over.
    series: usize,
    /// `--help` / `-h`.
    help: bool,
    /// `--version`.
    version: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            target: "127.0.0.1:7000".to_owned(),
            samples: 1000,
            batch: 128,
            seed: 1,
            series: 8,
            help: false,
            version: false,
        }
    }
}

impl Options {
    /// Read the command line.
    ///
    /// # Errors
    ///
    /// [`lakelet::Error::Invalid`] for an unknown flag, a missing value or a number that is not one —
    /// the shape the `exit-code` gate probes every binary with.
    fn from_args(args: impl Iterator<Item = String>) -> Result<Self, lakelet::Error> {
        let mut options = Self::default();
        let mut args = args;
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--target" => options.target = value(&mut args, "--target")?,
                "--samples" => options.samples = parsed(&mut args, "--samples")?,
                "--batch" => options.batch = parsed(&mut args, "--batch")?,
                "--seed" => options.seed = parsed(&mut args, "--seed")?,
                "--series" => options.series = parsed(&mut args, "--series")?,
                "--help" | "-h" => options.help = true,
                "--version" => options.version = true,
                other => {
                    return Err(lakelet::Error::Invalid {
                        detail: format!("unknown argument `{other}`"),
                    });
                }
            }
        }
        Ok(options)
    }
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, lakelet::Error> {
    args.next().ok_or_else(|| lakelet::Error::Invalid {
        detail: format!("`{flag}` needs a value"),
    })
}

fn parsed<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, lakelet::Error> {
    let text = value(args, flag)?;
    text.parse().map_err(|_| lakelet::Error::Invalid {
        detail: format!("`{flag}` takes a number, and `{text}` is not one"),
    })
}

/// The workload a run sends, and the only place a seed becomes bytes.
///
/// A tiny xorshift, not a dependency: the workload has to be the same on every machine and in every
/// process, and "the same PRNG as everyone else" is a property a course can check.
fn simulate(options: &Options) -> Vec<Record> {
    let mut state = options.seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..options.samples)
        .map(|index| Record {
            series: format!("cpu,host=h{}", (next() as usize) % options.series.max(1)),
            ts_millis: 1_700_000_000_000 + (index as i64) * 1000,
            value: (next() % 10_000) as f64 / 100.0,
        })
        .collect()
}
