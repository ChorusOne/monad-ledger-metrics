# Prometheus exporter for Monad's ledger metrics

This is a very simple tool that parses the JSON lines emitted by `ledger-tail` and emits prometheus metrics.

The tool must receive the output of `ledger-tail` in stdin, and the mandatory argument `--listen-addr`. 

Usage:

```
docker compose run --remove-orphans bft /usr/local/bin/ledger-tail | \
    ledger-exporter  --listen-addr 0.0.0.0:8001 --our-addresses 000000000000000000000000000000000000000000000000000000000000000000
```

Alternatively, add the exporter to the docker compose file:

```
# Exports mpt metrics
  ledger-exporter:
    image: ${REPO_URL}/monad-bft:${IMAGE_TAG}
    security_opt:
      - seccomp:./profile.json
    ports:
      - "8001:8001"
    volumes:
      - your-path-to-monad/monad-bft:/monad
      - your-path-to-ledger-metrics-exporter-binary:/ledger-exporter
    command: sh -c '/usr/local/bin/ledger-tail | /ledger-exporter --listen-addr 0.0.0.0:8001 --our-addresses 03a835aa1476e90d170655f333ef910e24be174dbd7d1b4a099f6a596ec6f176cb'
    networks:
      - monad
    devices:
      - /dev/${TARGET_DRIVE}:/dev/${TARGET_DRIVE}
    logging:
      driver: journald
      options:
        tag: "monad-bft"
    depends_on:
      mpt:
        condition: service_completed_successfully
        required: false
    environment:
      - RUST_LOG=debug,h2=warn,tower=warn,monad_statesync=trace
```

The log lines look like:
```
{"timestamp":"2025-05-08T17:39:50.022555Z","level":"INFO","fields":{"message":"proposed_block","round":"16652760","parent_round":"16652759","epoch":"319","seq_num":"15917009","num_tx":"55","author":"000000000000000000000000000000000000000000000000000000000000000000","block_ts_ms":"1746725989924","now_ts_ms":"1746725990022","author_dns":"monad-testnet.domain.com:8000"},"target":"ledger_tail"}
{"timestamp":"2025-05-08T17:36:06.379049Z","level":"INFO","fields":{"message":"skipped_block","round":"16641085","author":"000000000000000000000000000000000000000000000000000000000000000000","now_ts_ms":"1746725766374","author_dns":"monad-testnet.domain.com:8000"},"target":"ledger_tail"}
```

The exported metrics look like:

```
# HELP monad_ledger_exporter_lines_parsed Number of lines parsed by the ledger exporter
# TYPE monad_ledger_exporter_lines_parsed counter
monad_ledger_exporter_lines_parsed{status="failure"} 0
monad_ledger_exporter_lines_parsed{status="success"} 90608
# HELP monad_proposed_blocks Number of proposed blocks by author.
# TYPE monad_proposed_blocks counter
monad_proposed_blocks{author="000000000000000000000000000000000000000000000000000000000000000000",author_dns="monad-testnet.domain.com:8000",operated_by_us="false"} 843
# HELP monad_skipped_blocks Number of skipped blocks by author.
# TYPE monad_skipped_blocks counter
monad_skipped_blocks{author="000000000000000000000000000000000000000000000000000000000000000000",author_dns="monad-testnet.domain.com:8000",operated_by_us="false"} 7
```
