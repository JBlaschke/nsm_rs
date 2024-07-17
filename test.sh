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

./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 11000 &
listener_pid=$!

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 11000 --bind-port 11010 --service-port 11020 --key 5555 &
publisher_pid=$!

sleep 5

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

sleep 1

./test_program/target/debug/my_server "$claim_ip" "$claim_port" &
my_server_pid=$!

sleep 3

kill -9 $claimer_pid

sleep 3

kill -9 $publisher_pid

sleep 3

kill -9 $my_server_pid

sleep 3

kill -9 $listener_pid

wait


#sample listen + 2 publish + claim
#quit claim, then publish, then publish, then listen
# sleep 1

# echo "Running listen with 2 services and 1 client"
# ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
# listener_pid=$! 

# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
# publisher1_pid=$!

# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12020 --service-port 12021 --key 1234 &
# publisher2_pid=$!

# sleep 3
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
# claimer_pid=$!

# sleep 3

# kill -9 $claimer_pid

# sleep 3

# kill -9 $publisher2_pid

# sleep 3 

# kill -9 $publisher1_pid

# sleep 3 

# kill -9 $listener_pid

# wait

# #sample listen + 2 publish + 2 claim
# #quit publish, then claim, then listen

# sleep 1
# echo "Running listen with 2 services and 2 clients"
# ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
# listener_pid=$! 

# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
# publisher1_pid=$!

# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12020 --service-port 12021 --key 1234 &
# publisher2_pid=$!

# sleep 3
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
# claimer1_pid=$!

# sleep 3
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12025 --key 1234 &
# claimer2_pid=$!

# sleep 3

# kill -9 $claimer2_pid

# sleep 3

# kill -9 $claimer1_pid

# sleep 3

# kill -9 $publisher2_pid

# sleep 3 

# kill -9 $publisher1_pid

# sleep 3 

# kill -9 $listener_pid

# wait

# #sample listen + publish + 2 claims

# sleep 1
# echo "Running listen with 1 service and 2 clients"
# ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
# listener_pid=$! 

# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 12000 --bind-port 12010 --service-port 12011 --key 1234 &
# publisher1_pid=$!

# sleep 3
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
# claimer1_pid=$!

# sleep 3
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12025 --key 1234 &
# claimer2_pid=$!

# sleep 3

# kill -9 $claimer2_pid

# sleep 3

# kill -9 $claimer1_pid

# sleep 3 

# kill -9 $publisher1_pid

# sleep 3 

# kill -9 $listener_pid

# wait

# #sample listen + publish + 2 claims

# sleep 1
# echo "Running listen with 1 client"

# ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 12000 &
# listener_pid=$! 

# sleep 3 &
# ./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 12000 --bind-port 12015 --key 1234 &
# claimer_pid=$!

# sleep 3 

# kill -9 $listener_pid

# wait