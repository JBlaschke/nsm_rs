#!/bin/bash

set -euo pipefail

echo "Compiling program"
cargo build

echo "Running list_interfaces"
./target/debug/nsm -o list_interfaces

echo "Running list_ips for IP version 4"
ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

echo "Starting listener"

./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 11000 &
listener_pid=$!

sleep 15

kill -9 $listener_pid

wait