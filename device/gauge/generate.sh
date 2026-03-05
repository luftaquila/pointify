#!/bin/bash
# Gauge SVG generator - shell equivalent of customizer.html
# Usage: ./generate.sh [options] -o output.svg
#
# Options:
#   -M  Major tick count       (default: 5)
#   -m  Medium ticks per major (default: 1)
#   -s  Small ticks per medium (default: 4)
#   -v  Max value              (default: 100)
#   -c  Center text            (default: V)
#   -C  Center text size       (default: 125)
#   -r  Right text             (default: 91C4)
#   -R  Right text margin      (default: 140)
#   -o  Output file            (default: stdout)

set -euo pipefail

# Defaults
MAJOR_TICK_COUNT=5
MEDIUM_PER_MAJOR=1
SMALL_PER_MEDIUM=4
MAX_VALUE=100
CENTER_TEXT="V"
CENTER_TEXT_SIZE=125
RIGHT_TEXT="91C4"
RIGHT_TEXT_MARGIN=140
OUTPUT=""

while getopts "M:m:s:v:c:C:r:R:o:" opt; do
  case $opt in
    M) MAJOR_TICK_COUNT=$OPTARG ;;
    m) MEDIUM_PER_MAJOR=$OPTARG ;;
    s) SMALL_PER_MEDIUM=$OPTARG ;;
    v) MAX_VALUE=$OPTARG ;;
    c) CENTER_TEXT=$OPTARG ;;
    C) CENTER_TEXT_SIZE=$OPTARG ;;
    r) RIGHT_TEXT=$OPTARG ;;
    R) RIGHT_TEXT_MARGIN=$OPTARG ;;
    o) OUTPUT=$OPTARG ;;
    *) echo "Unknown option: -$opt" >&2; exit 1 ;;
  esac
done

# Constants
ARC_START=-36
ARC_END=36
ARC_SPAN=72
RADIUS=567
CX=494
CY=746

# Ensure minimum values
[ "$MAJOR_TICK_COUNT" -lt 2 ] && MAJOR_TICK_COUNT=2
[ "$MEDIUM_PER_MAJOR" -lt 0 ] && MEDIUM_PER_MAJOR=0
[ "$SMALL_PER_MEDIUM" -lt 0 ] && SMALL_PER_MEDIUM=0

MAJOR_DIVISIONS=$((MAJOR_TICK_COUNT - 1))

# XML escape
xml_escape() {
  printf '%s' "$1" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'
}

# round to 4 decimal places
round4() {
  printf '%.4f' "$1" | sed 's/\.0000$//' | sed 's/\(\.[0-9]*[1-9]\)0*$/\1/'
}

# Compute major tick angles
declare -a MAJOR_ANGLES
for ((i = 0; i <= MAJOR_DIVISIONS; i++)); do
  angle=$(echo "scale=10; $ARC_START + $i * ($ARC_SPAN / $MAJOR_DIVISIONS)" | bc -l)
  MAJOR_ANGLES+=("$(round4 "$angle")")
done

# Compute labels
declare -a LABELS
for ((i = 0; i <= MAJOR_DIVISIONS; i++)); do
  val=$(echo "scale=10; $MAX_VALUE * $i / $MAJOR_DIVISIONS" | bc -l)
  # Check if integer
  int_val=$(echo "$val" | sed 's/\..*//')
  frac=$(echo "scale=10; $val - $int_val" | bc -l)
  if echo "$frac" | grep -qE '^\.?0+$'; then
    LABELS+=("$int_val")
  else
    formatted=$(printf '%.1f' "$val" | sed 's/\.0$//')
    LABELS+=("$formatted")
  fi
done

# Compute medium tick angles
declare -a MEDIUM_ANGLES
if [ "$MEDIUM_PER_MAJOR" -gt 0 ]; then
  for ((i = 0; i < MAJOR_DIVISIONS; i++)); do
    a0=${MAJOR_ANGLES[$i]}
    a1=${MAJOR_ANGLES[$((i + 1))]}
    step=$(echo "scale=10; ($a1 - $a0) / ($MEDIUM_PER_MAJOR + 1)" | bc -l)
    for ((j = 1; j <= MEDIUM_PER_MAJOR; j++)); do
      angle=$(echo "scale=10; $a0 + $j * $step" | bc -l)
      MEDIUM_ANGLES+=("$(round4 "$angle")")
    done
  done
fi

# Compute small tick angles
declare -a SMALL_ANGLES
if [ "$SMALL_PER_MEDIUM" -gt 0 ]; then
  # Combine and sort major + medium angles
  ALL_BIG=("${MAJOR_ANGLES[@]}" "${MEDIUM_ANGLES[@]}")
  IFS=$'\n' SORTED=($(printf '%s\n' "${ALL_BIG[@]}" | sort -g)); unset IFS

  for ((i = 0; i < ${#SORTED[@]} - 1; i++)); do
    a0=${SORTED[$i]}
    a1=${SORTED[$((i + 1))]}
    step=$(echo "scale=10; ($a1 - $a0) / ($SMALL_PER_MEDIUM + 1)" | bc -l)
    for ((j = 1; j <= SMALL_PER_MEDIUM; j++)); do
      angle=$(echo "scale=10; $a0 + $j * $step" | bc -l)
      SMALL_ANGLES+=("$(round4 "$angle")")
    done
  done
fi

# Build SVG parts
build_small_lines() {
  for a in "${SMALL_ANGLES[@]}"; do
    echo "      <line y1=\"-567\" y2=\"-600\" transform=\"rotate($a)\" />"
  done
}

build_medium_lines() {
  for a in "${MEDIUM_ANGLES[@]}"; do
    echo "      <line y1=\"-567\" y2=\"-617\" transform=\"rotate($a)\" />"
  done
}

build_major_lines() {
  for a in "${MAJOR_ANGLES[@]}"; do
    echo "      <line y1=\"-567\" y2=\"-637\" transform=\"rotate($a)\" />"
  done
}

build_labels() {
  for ((i = 0; i < ${#MAJOR_ANGLES[@]}; i++)); do
    a=${MAJOR_ANGLES[$i]}
    text=$(xml_escape "${LABELS[$i]}")
    echo "      <text y=\"-655\" transform=\"rotate($a)\">$text</text>"
  done
}

RIGHT_TEXT_X=$((1000 - RIGHT_TEXT_MARGIN))
ESCAPED_CENTER=$(xml_escape "$CENTER_TEXT")
ESCAPED_RIGHT=$(xml_escape "$RIGHT_TEXT")

# Generate SVG
generate() {
cat << SVGEOF
<svg width="51.5mm" height="42.04mm" viewBox="-120.5 -120.5 1241 1013" xmlns="http://www.w3.org/2000/svg">
  <path d="
    M 0 0
    L 1000 0
    L 1000 772
    L 736 772
    L 736 620
    A 291 291 0 0 0 258 620
    L 258 772
    L 0 772
    Z
    M 129 608 A 27 27 0 1 0 75 608 A 27 27 0 1 0 129 608
    M 916 608 A 27 27 0 1 0 862 608 A 27 27 0 1 0 916 608
  " fill="none" stroke="gray" stroke-width="2" fill-rule="evenodd" />

  <g transform="translate($CX, $CY)">

    <path d="M -336.7 -456.2 A $RADIUS $RADIUS 0 0 1 336.7 -456.2" fill="none" stroke="#111" stroke-width="9" stroke-linecap="butt" />

SVGEOF

if [ ${#SMALL_ANGLES[@]} -gt 0 ]; then
  echo '    <g stroke="#111" stroke-width="5.5" stroke-linecap="butt">'
  build_small_lines
  echo '    </g>'
  echo ''
fi

if [ ${#MEDIUM_ANGLES[@]} -gt 0 ]; then
  echo '    <g stroke="#111" stroke-width="6.5" stroke-linecap="butt">'
  build_medium_lines
  echo '    </g>'
  echo ''
fi

cat << SVGEOF
    <g stroke="#111" stroke-width="9" stroke-linecap="butt">
$(build_major_lines)
    </g>

    <g font-family="Arial, Helvetica, sans-serif" font-size="70" font-weight="bold" fill="#111" text-anchor="middle">
$(build_labels)
    </g>
  </g>

  <text x="$CX" y="370" font-family="Arial, Helvetica, sans-serif" font-size="$CENTER_TEXT_SIZE" font-weight="bold" fill="#111" text-anchor="middle">$ESCAPED_CENTER</text>

  <text x="$RIGHT_TEXT_X" y="480" font-family="Arial, Helvetica, sans-serif" font-size="62" font-weight="bold" fill="#111" text-anchor="middle" letter-spacing="1">$ESCAPED_RIGHT</text>

  <g fill="#111">
    <text x="42" y="477" font-family="Arial, Helvetica, sans-serif" font-size="62" font-weight="bold">&#x2212;</text>
    <g transform="translate(92, 426)" fill="none" stroke="#111" stroke-width="8">
      <path d="M 0 52 L 0 22 A 14 14 0 0 1 28 22 L 28 52" stroke-linecap="butt"/>
      <line x1="8" y1="49.5" x2="20" y2="49.5" stroke-width="5"/>
    </g>
    <text x="135" y="477" font-family="Arial, Helvetica, sans-serif" font-size="62" font-weight="bold">2.5</text>
    <g transform="translate(233, 438)">
      <path d="M 21 0 L 0 36 L 42 36 Z" fill="none" stroke="#111" stroke-width="6" stroke-linejoin="miter"/>
      <line x1="21" y1="13" x2="21" y2="25" stroke="#111" stroke-width="5" stroke-linecap="butt"/>
      <circle cx="21" cy="30" r="3" fill="#111"/>
    </g>
  </g>

  <line x1="42" y1="510" x2="280" y2="510" stroke="#111" stroke-width="6" stroke-linecap="butt" />

</svg>
SVGEOF
}

if [ -n "$OUTPUT" ]; then
  generate > "$OUTPUT"
  echo "Generated: $OUTPUT"
else
  generate
fi
