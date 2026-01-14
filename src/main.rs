use std::{
    collections::HashMap,
    io::BufRead,
    net::SocketAddr,
    process::{Command, Stdio},
    str::FromStr,
};

use clap::Parser;
use prometheus::{default_registry, register_int_counter_vec, Encoder, TextEncoder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
struct Identity {
    addr: String,
    name: String,
}

impl FromStr for Identity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr, name) = s
            .split_once(':')
            .ok_or_else(|| format!("invalid identity {s}, expected name:addr"))?;

        if name.is_empty() || addr.is_empty() {
            return Err(format!(
                "invalid identity {s}, name and addr must be non-empty"
            ));
        }

        Ok(Identity {
            addr: addr.to_string(),
            name: name.to_string(),
        })
    }
}

#[derive(Parser, Debug)]
struct Opt {
    /// Address for the Prometheus exporter to listen on
    #[arg(long)]
    listen_addr: String,

    /// Path to the monad-ledger-tail binary
    #[arg(long, default_value = "monad-ledger-tail")]
    ledger_tail_bin: String,

    /// Extra args to pass to monad-ledger-tail (space-separated)
    ///   --ledger-tail-args "--ledger-path=/opt/validators/monad/monad-bft/ledger --forkpoint-path=/opt/validators/monad/monad-bft/config/forkpoint/forkpoint.toml"
    #[arg(long = "ledger-tail-args", default_value = "", allow_hyphen_values = true)]
    ledger_tail_args: String,

    /// Mapping a secp pubkey to a human-friendly name
    ///   --known-identity addressblablabla:chorus1 --known-identity addressblebleble:chorus2
    #[arg(long = "known-identity")]
    known_identities: Vec<Identity>,
}

impl Opt {
    fn known_identities_map(&self) -> HashMap<String, String> {
        self.known_identities
            .iter()
            .map(|id| (id.addr.clone(), id.name.clone()))
            .collect()
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub fields: LogFields,
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "message")]
pub enum LogFields {
    #[serde(rename = "proposed_block")]
    ProposedBlock {
        round: String,
        author: String,
        now_ts_ms: String,
        author_dns: Option<String>,
        author_address: Option<String>,
        // unique fields
        epoch: String,
        seq_num: String,
        num_tx: String,
        block_ts_ms: String,
    },
    #[serde(rename = "skipped_block")]
    SkippedBlock {
        round: String,
        author: String,
        now_ts_ms: String,
        author_dns: Option<String>,
        author_address: Option<String>,
    },
    #[serde(rename = "finalized_block")]
    FinalizedBlock {
        round: String,
        author: String,
        now_ts_ms: String,
        author_dns: Option<String>,
        author_address: Option<String>,
        epoch: String,
        seq_num: String,
        block_ts_ms: String,
    },
    #[serde(rename = "timeout")]
    Timeout {
        round: String,
        author: String,
        now_ts_ms: String,
        author_dns: Option<String>,
        author_address: Option<String>,
    },
}

fn main() -> std::io::Result<()> {
    let opt = Opt::parse();
    let addr: SocketAddr = opt.listen_addr.parse().expect("Invalid listen-addr");

    let _jh = std::thread::spawn(move || serve(&addr));

    let ledger_tail_args = opt
        .ledger_tail_args
        .split_whitespace()
        .filter(|arg| !arg.is_empty())
        .collect::<Vec<_>>();

    let mut child = Command::new(&opt.ledger_tail_bin)
        .args(ledger_tail_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|err| {
            panic!(
                "failed to start monad-ledger-tail at {}: {err}",
                opt.ledger_tail_bin
            )
        });

    println!(
        "Started monad-ledger-tail (pid {})",
        child.id()
    );
    if let Ok(Some(status)) = child.try_wait() {
        eprintln!("monad-ledger-tail exited immediately with status {status}");
        std::process::exit(1);
    }

    let stdout = child
        .stdout
        .take()
        .expect("failed to capture monad-ledger-tail stdout");

    let known: HashMap<String, String> = opt.known_identities_map();
    let sh = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        parse_reader(reader, known)
    });

    let status = child
        .wait()
        .expect("failed to wait for monad-ledger-tail");
    let _ = sh.join();

    if !status.success() {
        eprintln!("monad-ledger-tail exited with status {status}");
    } else {
        eprintln!("monad-ledger-tail exited; shutting down exporter");
    }
    std::process::exit(1);
}

fn parse_reader<R: BufRead>(
    reader: R,
    our_addresses: HashMap<String, String>,
) -> std::io::Result<()> {
    println!("Parsing metrics, our addresses are {our_addresses:?}");
    let proposed_blocks = register_int_counter_vec!(
        "monad_proposed_blocks",
        "Number of proposed blocks by author.",
        &[
            "author",
            "author_dns",
            "author_address",
            "operated_by_us",
            "validator_name"
        ]
    )
    .unwrap();

    let skipped_blocks = register_int_counter_vec!(
        "monad_skipped_blocks",
        "Number of skipped blocks by author.",
        &[
            "author",
            "author_dns",
            "author_address",
            "operated_by_us",
            "validator_name"
        ]
    )
    .unwrap();

    let read_lines = register_int_counter_vec!(
        "monad_ledger_exporter_lines_parsed",
        "Number of lines parsed by the ledger exporter",
        &["status"],
    )
    .unwrap();

    read_lines.with_label_values(&["success"]).reset();
    read_lines.with_label_values(&["failure"]).reset();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<LogEntry>(&line) {
            Ok(log_entry) => {
                read_lines.with_label_values(&["success"]).inc();
                match log_entry.fields {
                    LogFields::ProposedBlock {
                        author,
                        author_dns,
                        author_address,
                        ..
                    } => {
                        let (operated_by_us, validator_name) = match our_addresses.get(&author) {
                            Some(name) => ("true", name.as_str()),
                            None => ("false", ""),
                        };

                        proposed_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
                                validator_name,
                            ])
                            .inc();
                    }
                    LogFields::SkippedBlock {
                        author,
                        author_dns,
                        author_address,
                        ..
                    } => {
                        let (operated_by_us, validator_name) = match our_addresses.get(&author) {
                            Some(name) => ("true", name.as_str()),
                            None => ("false", ""),
                        };

                        skipped_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
                                validator_name,
                            ])
                            .inc();
                    }
                    LogFields::FinalizedBlock { .. } => {}
                    LogFields::Timeout {
                        author,
                        author_dns,
                        author_address,
                        ..
                    } => {
                        let (operated_by_us, validator_name) = match our_addresses.get(&author) {
                            Some(name) => ("true", name.as_str()),
                            None => ("false", ""),
                        };

                        skipped_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
                                validator_name,
                            ])
                            .inc();
                    }
                }
            }
            Err(e) => {
                read_lines.with_label_values(&["failure"]).inc();
                eprintln!("Error parsing line: {}", e);
                eprintln!("Problematic line: {}", line);
            }
        }
    }
    Ok(())
}

fn serve(address: &SocketAddr) {
    let encoder = TextEncoder::new();
    let registry = default_registry();
    println!("Starting metrics server on {address:?}");
    let server = tiny_http::Server::http(address).expect("Unable to bind to address");
    for request in server.incoming_requests() {
        let mut response = Vec::<u8>::new();
        let metric_families = registry.gather();
        encoder.encode(&metric_families, &mut response).unwrap();
        request
            .respond(tiny_http::Response::from_data(response))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_three_kinds() {
        let proposed = r#"{"timestamp":"2025-08-29T13:10:36.585635Z","level":"INFO","fields":{"message":"proposed_block","round":"35507367","parent_round":"35507366","epoch":"677","seq_num":"33808561","num_tx":"29","author":"029efe69e22c0f7244e6566ad73537c3827801cd75da425f91235890da36888c9b","block_ts_ms":"1756473036514","now_ts_ms":"1756473036575","author_address":"84.32.220.55:8000"},"target":"ledger_tail"}"#;
        let finalized = r#"{"timestamp":"2025-08-29T13:10:36.585640Z","level":"INFO","fields":{"message":"finalized_block","round":"35507364","parent_round":"35507363","epoch":"677","seq_num":"33808558","author":"02c34fa55bf2b2a80e3d562afb02710d19119ae02e5f079a2940bde57dadc3029f","block_ts_ms":"1756473035364","now_ts_ms":"1756473036575","author_address":"202.8.11.136:8000"},"target":"ledger_tail"}"#;
        let skipped = r#"{"timestamp":"2025-07-31T07:36:27.794234Z","level":"INFO","fields":{"message":"skipped_block","round":"6622784","author":"03de1f49f8a52a2f62196a6f88c436a37e5d3cd88b37588f4cab8f1dcdbf18148e","now_ts_ms":"1753947387794","author_dns":"64.130.43.22:8000"},"target":"ledger_tail"}"#;

        let e1: LogEntry = serde_json::from_str(proposed).unwrap();
        let e2: LogEntry = serde_json::from_str(finalized).unwrap();
        let e3: LogEntry = serde_json::from_str(skipped).unwrap();

        assert_eq!(e1.level, "INFO");
        assert_eq!(e1.target, "ledger_tail");
        match e1.fields {
            LogFields::ProposedBlock {
                round,
                epoch,
                seq_num,
                num_tx,
                author,
                block_ts_ms,
                now_ts_ms,
                author_dns,
                author_address,
            } => {
                assert_eq!(round, "35507367");
                assert_eq!(epoch, "677");
                assert_eq!(seq_num, "33808561");
                assert_eq!(num_tx, "29");
                assert_eq!(
                    author,
                    "029efe69e22c0f7244e6566ad73537c3827801cd75da425f91235890da36888c9b"
                );
                assert_eq!(block_ts_ms, "1756473036514");
                assert_eq!(now_ts_ms, "1756473036575");
                assert_eq!(author_dns, None);
                assert_eq!(author_address, Some("84.32.220.55:8000".into()));
            }
            _ => panic!("expected ProposedBlock"),
        }

        assert_eq!(e2.level, "INFO");
        assert_eq!(e2.target, "ledger_tail");
        match e2.fields {
            LogFields::FinalizedBlock {
                round,
                epoch,
                seq_num,
                author,
                block_ts_ms,
                now_ts_ms,
                author_dns,
                author_address,
            } => {
                assert_eq!(round, "35507364");
                assert_eq!(epoch, "677");
                assert_eq!(seq_num, "33808558");
                assert_eq!(
                    author,
                    "02c34fa55bf2b2a80e3d562afb02710d19119ae02e5f079a2940bde57dadc3029f"
                );
                assert_eq!(block_ts_ms, "1756473035364");
                assert_eq!(now_ts_ms, "1756473036575");
                assert_eq!(author_dns, None);
                assert_eq!(author_address, Some("202.8.11.136:8000".into()));
            }
            _ => panic!("expected FinalizedBlock"),
        }

        assert_eq!(e3.level, "INFO");
        assert_eq!(e3.target, "ledger_tail");
        match e3.fields {
            LogFields::SkippedBlock {
                round,
                author,
                now_ts_ms,
                author_dns,
                author_address,
            } => {
                assert_eq!(round, "6622784");
                assert_eq!(
                    author,
                    "03de1f49f8a52a2f62196a6f88c436a37e5d3cd88b37588f4cab8f1dcdbf18148e"
                );
                assert_eq!(now_ts_ms, "1753947387794");
                assert_eq!(author_dns, Some("64.130.43.22:8000".into()));
                assert_eq!(author_address, None);
            }
            _ => panic!("expected SkippedBlock"),
        }
    }
}
