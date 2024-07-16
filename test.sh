#!/bin/bash

set -euo pipefail

echo "Compiling program"
cargo build

#sample list_interfaces
echo "Running list_interfaces"
./target/debug/nsm -o list_interfaces

#sample list_ips
echo "Running list_ips for IP version 4"
ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

#sample listen + publish + claim
#quit listen 
echo "Running listen with publish and claim"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
listener_pid=$!

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12020 --key 1234 &
publisher_pid=$!

sleep 5
claim_output = $(./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12011 --key 1234 &)
claimer_pid=$!

sleep 1

IFS=$'\n' read -r -d '' -a claim_lines <<< "$claim_output"
service_addr="${claim_lines[0]}"
service_port="${claim_lines[1]}"

echo "Service Address: $service_addr"
echo "Service Port: $service_port"

sleep 3

kill -9 $claimer_pid

sleep 3

kill -9 $publisher_pid

sleep 3

kill -9 $listener_pid

wait


#sample listen + 2 publish + claim
#quit claim, then publish, then publish, then listen
sleep 1

echo "Running listen with 2 services and 1 client"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
listener_pid=$! 

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
publisher1_pid=$!

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12020 --service-port 12021 --key 1234 &
publisher2_pid=$!

sleep 3
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
claimer_pid=$!

sleep 3

kill -9 $claimer_pid

sleep 3

kill -9 $publisher2_pid

sleep 3 

kill -9 $publisher1_pid

sleep 3 

kill -9 $listener_pid

wait

#sample listen + 2 publish + 2 claim
#quit publish, then claim, then listen

sleep 1
echo "Running listen with 2 services and 2 clients"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
listener_pid=$! 

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
publisher1_pid=$!

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12020 --service-port 12021 --key 1234 &
publisher2_pid=$!

sleep 3
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
claimer1_pid=$!

sleep 3
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12025 --key 1234 &
claimer2_pid=$!

sleep 3

kill -9 $claimer2_pid

sleep 3

kill -9 $claimer1_pid

sleep 3

kill -9 $publisher2_pid

sleep 3 

kill -9 $publisher1_pid

sleep 3 

kill -9 $listener_pid

wait

#sample listen + publish + 2 claims

sleep 1
echo "Running listen with 1 service and 2 clients"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
listener_pid=$! 

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
publisher1_pid=$!

sleep 3
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
claimer1_pid=$!

sleep 3
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12025 --key 1234 &
claimer2_pid=$!

sleep 3

kill -9 $claimer2_pid

sleep 3

kill -9 $claimer1_pid

sleep 3 

kill -9 $publisher1_pid

sleep 3 

kill -9 $listener_pid

wait

#sample listen + publish + 2 claims

sleep 1
echo "Running listen with 1 client"

./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
listener_pid=$! 

sleep 3 &
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
claimer_pid=$!

sleep 3 

kill -9 $listener_pid

wait