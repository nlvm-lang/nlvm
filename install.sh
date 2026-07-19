#!/usr/bin/env bash

set -e

PWD=$(pwd)

cargo build -r

mkdir -p ~/.local/bin
ln -sf "$PWD/target/release/nlc" ~/.local/bin/nlc
ln -sf "$PWD/target/release/nlvm" ~/.local/bin/nlvm