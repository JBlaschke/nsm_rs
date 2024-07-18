#!/bin/bash

set -euo pipefail

echo "Compiling program"
cargo build

echo "Running list_interfaces"
./target/debug/nsm -o list_interfaces

echo "Running list_ips for IP version 4"
ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

echo "Starting client"

sleep 3

output_file=$(mktemp)
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 11000 --bind-port 11015 --key 5555 > $output_file &
claimer_pid=$!

sleep 2

claim_output=$(cat $output_file)

claim_ip=$(echo $claim_output | jq -r .service_addr[0])
claim_port=$(echo $claim_output | jq .service_port)

echo $claim_ip
echo $claim_port

rm $output_file

./test_program/target/debug/my_client "$claim_ip" "$claim_port" &
my_client_pid=$!

sleep 3

kill -9 $my_client_pid

sleep 3

kill -9 $claimer_pid

wait