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
#quit claim, then publish, then listen

echo "Running listen with publish and claim"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 8000 &

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 8000 --bind-port 8010 --service-port 8020 --key 1234 &

sleep 1 &
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 8000 --bind-port 8011 --key 1234 &

sleep 3

kill -9 %4

sleep 3

kill -9 %2 

sleep 3 

kill -9 %1 

wait


#sample listen + publish + claim
#quit publish, then claim, then listen
echo "Running listen with publish and claim"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 8000 &

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 8000 --bind-port 8010 --service-port 8020 --key 1234 &

sleep 1 &
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 8000 --bind-port 8011 --key 1234 &

sleep 3

kill -9 %2

sleep 3

kill -9 %4 

sleep 3 

kill -9 %1 

wait

#sample listen + publish + publish + claim
#quit claim, then publish, then listen
echo "Running listen with publish and claim"
./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 8000 &

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 8000 --bind-port 8010 --service-port 8011 --key 1234 &

./target/debug/nsm -n en0 --ip-start "$ip_address" --operation publish --host "$ip_address" --port 8000 --bind-port 8020 --service-port 8021 --key 1234 &

sleep 1 &
./target/debug/nsm -n en0 --ip-start "$ip_address" --operation claim --host "$ip_address" --port 8000 --bind-port 8015 --key 1234 &

sleep 3

kill -9 %2

sleep 3

kill -9 %4 

sleep 3 

kill -9 %1 

wait

#sample listen + publish + 2 claims