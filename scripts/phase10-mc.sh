#!/bin/bash
# usage: mc.sh scenario [flags...] -> prints one line with verdict/states/transitions/ms/rss
SC=$1; shift
BIN=${BIN:-./target/release/commitrift}
OUT=$( { /usr/bin/time -l $BIN model-check $SC --format json "$@" 2>/tmp/mc_err.txt; } )
RSS=$(grep "maximum resident" /tmp/mc_err.txt | awk '{printf "%.1f MiB", $1/1048576}')
echo "$OUT" | python3 -c "
import json,sys
d=json.load(sys.stdin); e=d['exploration']; m=e['metrics']; t=d.get('timing',{})
print(e['verdict'], 'states',m['states'],'transitions',m['transitions'],'wallMs',t.get('wallMs'),'terminals',m['terminalStates'], 'rm', e.get('reductionMetrics'))
" | tr '\n' ' '
echo " rss=$RSS"
