#!/bin/bash

set -euo pipefail

echo "Compiling program"
cargo build

echo "Running list_interfaces"
./target/debug/nsm -o list_interfaces

echo "Running list_ips for IP version 4"
ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

echo "Starting server"

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 11000 --bind-port 11010 --service-port 11020 --key 5555 &
publisher_pid=$!

./test_program/target/debug/my_server "$ip_address" 11020 &
my_server_pid=$!

sleep 10

kill -9 $my_server_pid

sleep 3

kill -9 $publisher_pid

wait
