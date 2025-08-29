use std::{io::BufRead, net::SocketAddr};

use prometheus::{default_registry, register_int_counter_vec, Encoder, TextEncoder};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Opt {
    /// Address for the Prometheus exporter to listen on
    #[structopt(long)]
    listen_addr: String,

    /// Addresses that you operate. Metrics will have `operated_by_us=true` for
    /// events matching these addresses.
    #[structopt(long)]
    our_addresses: Vec<String>,
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
    let opt: Opt = Opt::from_args();
    let addr: SocketAddr = opt.listen_addr.parse().expect("Invalid listen-addr");

    let jh = std::thread::spawn(move || serve(&addr));
    let sh = std::thread::spawn(move || parse_stdin(&opt.our_addresses));

    jh.join().unwrap();
    sh.join().unwrap().unwrap();

    Ok(())
}

fn parse_stdin(our_addresses: &[String]) -> std::io::Result<()> {
    println!("Parsing metrics, our addresses are {our_addresses:?}");
    let proposed_blocks = register_int_counter_vec!(
        "monad_proposed_blocks",
        "Number of proposed blocks by author.",
        &["author", "author_dns", "author_address", "operated_by_us"]
    )
    .unwrap();
    let skipped_blocks = register_int_counter_vec!(
        "monad_skipped_blocks",
        "Number of skipped blocks by author.",
        &["author", "author_dns", "author_address", "operated_by_us"]
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

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
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
                        let operated_by_us: &str = if our_addresses.contains(&author) {
                            "true"
                        } else {
                            "false"
                        };
                        proposed_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
                            ])
                            .inc();
                    }
                    LogFields::SkippedBlock {
                        author,
                        author_dns,
                        author_address,
                        ..
                    } => {
                        let operated_by_us: &str = if our_addresses.contains(&author) {
                            "true"
                        } else {
                            "false"
                        };
                        skipped_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
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
                        let operated_by_us: &str = if our_addresses.contains(&author) {
                            "true"
                        } else {
                            "false"
                        };
                        skipped_blocks
                            .with_label_values(&[
                                author.as_str(),
                                author_dns.as_deref().unwrap_or(""),
                                author_address.as_deref().unwrap_or(""),
                                operated_by_us,
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
