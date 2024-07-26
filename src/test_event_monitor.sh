#!/bin/bash

set -euo pipefail

ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

NSM_LOG_LEVEL=trace ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 11000 &
listener_pid=$!


sleep 1
./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11010 --service-port 11020 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11011 --service-port 11021 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11012 --service-port 11022 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11013 --service-port 11023 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11014 --service-port 11024 --key 1234 &

# for i in {1..10}
# do
#     sleep 1
#     ./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11030 --service-port 11040 --key 5678 &
# done


sleep 1
./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11030 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11031 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11032 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11033 --key 1234 &

sleep 1
./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11034 --key 1234 &

# for i in {1..10}
# do
#     sleep 1
#     ./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11025 --key 5678 &
# done 

sleep 60

kill -9 $listener_pid

wait