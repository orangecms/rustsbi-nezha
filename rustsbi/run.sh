#!/bin/sh

export SBIELF=../target/riscv64imac-unknown-none-elf/release/rustsbi-nezha
export SBIBIN=rustsbi

cargo build --release
riscv64-unknown-elf-objcopy -O binary $SBIELF $SBIBIN
xfel ddr ddr3
xfel write 0x40100000 "$SBIBIN"
xfel exec  0x40100000
