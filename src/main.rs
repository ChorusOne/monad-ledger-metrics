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
#[serde(untagged)]
pub enum LogFields {
    ProposedBlock(ProposedBlockFields),
    SkippedBlock(SkippedBlockFields),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProposedBlockFields {
    pub message: String,
    pub round: String,
    pub author: String,
    pub now_ts_ms: String,
    pub author_dns: String,
    // unique fields
    pub epoch: String,
    pub seq_num: String,
    pub num_tx: String,
    pub block_ts_ms: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SkippedBlockFields {
    pub message: String,
    pub round: String,
    pub author: String,
    pub now_ts_ms: String,
    pub author_dns: String,
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
        &["author", "author_dns", "operated_by_us"]
    )
    .unwrap();
    let skipped_blocks = register_int_counter_vec!(
        "monad_skipped_blocks",
        "Number of skipped blocks by author.",
        &["author", "author_dns", "operated_by_us"]
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
                match &log_entry.fields {
                    LogFields::ProposedBlock(fields) => {
                        proposed_blocks
                            .with_label_values(&[
                                fields.author.clone(),
                                fields.author_dns.clone(),
                                our_addresses.contains(&fields.author).to_string(),
                            ])
                            .inc();
                    }
                    LogFields::SkippedBlock(fields) => {
                        skipped_blocks
                            .with_label_values(&[
                                fields.author.clone(),
                                fields.author_dns.clone(),
                                our_addresses.contains(&fields.author).to_string(),
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
    fn test_parse_skipped_block() {
        let json = r#"{"timestamp":"2025-07-31T07:36:27.794234Z","level":"INFO","fields":{"message":"skipped_block","round":"6622784","author":"03de1f49f8a52a2f62196a6f88c436a37e5d3cd88b37588f4cab8f1dcdbf18148e","now_ts_ms":"1753947387794","author_dns":"64.130.43.22:8000"},"target":"ledger_tail"}"#;

        let log_entry: LogEntry = serde_json::from_str(json).unwrap();

        assert_eq!(log_entry.timestamp, "2025-07-31T07:36:27.794234Z");
        assert_eq!(log_entry.level, "INFO");
        assert_eq!(log_entry.target, "ledger_tail");

        match log_entry.fields {
            LogFields::SkippedBlock(fields) => {
                assert_eq!(fields.message, "skipped_block");
                assert_eq!(fields.round, "6622784");
                assert_eq!(
                    fields.author,
                    "03de1f49f8a52a2f62196a6f88c436a37e5d3cd88b37588f4cab8f1dcdbf18148e"
                );
                assert_eq!(fields.now_ts_ms, "1753947387794");
                assert_eq!(fields.author_dns, "64.130.43.22:8000");
            }
            _ => panic!("Expected SkippedBlock fields"),
        }
    }

    #[test]
    fn test_parse_proposed_block() {
        let json = r#"{"timestamp":"2025-07-31T07:36:27.794242Z","level":"INFO","fields":{"message":"proposed_block","round":"6622785","epoch":"122","seq_num":"6081798","num_tx":"0","author":"02d1e8a85c90d37799387cbfe7c53b45f24d4dca5674553bd442f36e45fdbb5b91","block_ts_ms":"1753947382250","now_ts_ms":"1753947387794","author_dns":"84.32.103.144:8000"},"target":"ledger_tail"}"#;

        let log_entry: LogEntry = serde_json::from_str(json).unwrap();

        assert_eq!(log_entry.timestamp, "2025-07-31T07:36:27.794242Z");
        assert_eq!(log_entry.level, "INFO");
        assert_eq!(log_entry.target, "ledger_tail");

        match log_entry.fields {
            LogFields::ProposedBlock(fields) => {
                assert_eq!(fields.message, "proposed_block");
                assert_eq!(fields.round, "6622785");
                assert_eq!(fields.epoch, "122");
                assert_eq!(fields.seq_num, "6081798");
                assert_eq!(fields.num_tx, "0");
                assert_eq!(
                    fields.author,
                    "02d1e8a85c90d37799387cbfe7c53b45f24d4dca5674553bd442f36e45fdbb5b91"
                );
                assert_eq!(fields.block_ts_ms, "1753947382250");
                assert_eq!(fields.now_ts_ms, "1753947387794");
                assert_eq!(fields.author_dns, "84.32.103.144:8000");
            }
            _ => panic!("Expected ProposedBlock fields"),
        }
    }
}
