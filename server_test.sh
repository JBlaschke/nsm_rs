#!/bin/bash

set -euo pipefail

ip_address=$(echo ) #fill in with listener address
echo "Starting server"

./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11010 --service-port 11020 --key 5555 &
publisher_pid=$!

./test_program/target/debug/my_server "$ip_address" 11020 &
my_server_pid=$!

sleep 10

kill -9 $my_server_pid

sleep 3

kill -9 $publisher_pid

wait
