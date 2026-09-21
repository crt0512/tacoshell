#!/bin/sh
# fill.sh VALUES TEMPLATE: replaces every @KEY@ in TEMPLATE with KEY's value from the KEY=value lines in VALUES. 
#literal, no regex, so & | / in a description cant break it.
# a later line for the same KEY wins. lines left as "Key: " or "Key=" (empty value) are dropped,
# so optional fields just disappear
awk '
NR == FNR {
    i = index($0, "=")
    if (i) v[substr($0, 1, i - 1)] = substr($0, i + 1)
    next
}
{
    for (k in v) {
        p = "@" k "@"
        while ((j = index($0, p)) > 0) $0 = substr($0, 1, j - 1) v[k] substr($0, j + length(p))
    }
    if ($0 ~ /^[A-Za-z-]+: *$/ || $0 ~ /^[A-Za-z]+=$/) next
    print
}' "$1" "$2"
