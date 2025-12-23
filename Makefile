.PHONY: fan-in fan-out ui

DURATION ?= 60s
QOS ?= 2

fan-in:
	ulimit -n 20000 && cargo run --release -- run fan-in \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 2000 --subscribers 100 --rate 10

fan-out:
	ulimit -n 20000 && cargo run --release -- run fan-out \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 100 --subscribers 2000 --rate 1

ui:
	cargo run --release serve
