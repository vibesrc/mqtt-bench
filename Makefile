.PHONY: build fan-in fan-out ui

DURATION ?= 10m
QOS ?= 2
TARGET_DIR ?= bin

build:
	cargo build --release --target-dir $(TARGET_DIR)

fan-in: build
	ulimit -n 20000 && $(TARGET_DIR)/release/mqtt-bench run fan-in \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 2000 --subscribers 100 --rate 10

fan-out: build
	ulimit -n 20000 && $(TARGET_DIR)/release/mqtt-bench run fan-out \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 100 --subscribers 3000 --rate 1

ui: build
	$(TARGET_DIR)/release/mqtt-bench serve
