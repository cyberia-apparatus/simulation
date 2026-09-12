#!/bin/bash
# sweep 1: the loop-coupling question at 60k nodes, 2 seeds per g
cd ~/cyber/simulation
for g in 0.0 0.25 0.5 0.75 1.0; do
  for seed in 7 13; do
    out="runs/g${g}_s${seed}.jsonl"
    ./target/release/simulation --nodes 60000 --g $g --q 0.5 --d 64 \
      --compile-every 8000 --probe 16000 --seen-probe 200 --seed $seed > "$out" 2>&1
    echo "done $out"
  done
done
echo ALL_DONE
