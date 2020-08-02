#!/bin/bash

set -euxo pipefail

cargo build --release

until ./target/release/fleet-manager | tee -a $(date -u +"%Y-%m-%dT%H_%M_%SZ").log; do
    echo "Failed :("
    sleep 5
done
