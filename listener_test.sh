#!/bin/bash

set -euo pipefail

echo "Running list_ips for IP version 4"
ip_address=$(./target/debug/nsm -n hsn0 -o list_ips --ip-version 4 | tee /dev/tty)

echo "Starting listener"

./target/debug/nsm -n en0 --ip-version 4 --operation listen --bind-port 11000 &
listener_pid=$!

sleep 60

kill -9 $listener_pid

wait