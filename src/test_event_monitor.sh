#!/bin/bash

set -euo pipefail

ip_address=$(./target/debug/nsm -n en0 -o list_ips --ip-version 4 | tee /dev/tty)

NSM_LOG_LEVEL=trace ./target/debug/nsm -n en0 --ip-version 4 --ip-start "$ip_address" --operation listen --bind-port 11000 &
listener_pid=$!

# sleep 1
# ./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port 11010 --service-port 11011 --key 1234 &

# sleep 1
# ./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port 11020 --key 1234 &

for i in {1..10}
do
    bind_port=$((11000 + i))
    sleep 1
    ./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port "$bind_port" --service-port 11070 --key 1234 &
done

# for i in {1..10}
# do
#     bind_port=$((11010 + i))
#     sleep 1
#     ./target/debug/nsm -n en0 --operation publish --host "$ip_address" --port 11000 --bind-port "$bind_port" --service-port 11080 --key 5678 &
# done

for i in {1..10}
do
    bind_port=$((11050 + i))
    sleep 1
    ./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port "$bind_port" --key 1234 &
done 

# for i in {1..10}
# do
#     bind_port=$((11060 + i))
#     sleep 1
#     ./target/debug/nsm -n en0 --operation claim --host "$ip_address" --port 11000 --bind-port "$bind_port" --key 5678 &
# done 

sleep 60

kill -9 $listener_pid

wait