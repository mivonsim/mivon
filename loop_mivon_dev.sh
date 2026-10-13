#!/bin/bash
# Loop for continuous Mivon HDL (.mv) development
# Purpose: mivon-mv-dev
# Interval: 5m (300 seconds)

PURPOSE="mivon-mv-dev"
INTERVAL=300

PROMPT='Continue Mivon HDL (.mv) development:
1. Check what'"'"'s next in ROADMAP.md and IEEE_1800_MATRIX.md
2. Implement missing .mv features or fix bugs
3. Run tests to verify
4. Commit and push changes

Current status: F85 (Full SVA) done. Next priorities from ROADMAP.md:
- Fix mivon-parser bug: multiple modules in 1 SV file -> "expected endmodule"
- Complete .mv features: randc, soft constraint, ref/out ports
- Performance: SIMD lexer, parallel parsing (Fase 1-3)
- LSP/Diagnostics (Fase 4)
- More test coverage

Work on ONE logical change at a time. After each change: test -> commit -> push.'

echo "Starting loop for $PURPOSE every ${INTERVAL}s"
echo "First run will execute immediately, then every $INTERVAL seconds"

# Run once immediately
echo "AGENT_LOOP_TICK_$PURPOSE {\"prompt\":\"$PROMPT\"}"

# Then loop
while true; do
    sleep $INTERVAL
    echo "AGENT_LOOP_TICK_$PURPOSE {\"prompt\":\"$PROMPT\"}"
done