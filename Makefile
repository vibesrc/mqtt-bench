.PHONY: build fan-in fan-out ui

DURATION ?= 6h
QOS ?= 2
TARGET_DIR ?= bin

build:
	cargo build --release --target-dir $(TARGET_DIR)

fan-in: build
	$(TARGET_DIR)/release/mqtt-bench --db ./fan-in.duckdb \
	  run fan-in --client-prefix fanin --base-topic bench/fanin \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 1500 --subscribers 100 --rate 10

fan-out: build
	$(TARGET_DIR)/release/mqtt-bench --db ./fan-out.duckdb \
	  run fan-out --client-prefix fanout --base-topic bench/fanout \
		--duration $(DURATION) --qos $(QOS) \
		--publishers 100 --subscribers 1500 --rate 10

ui: build
	$(TARGET_DIR)/release/mqtt-bench serve
