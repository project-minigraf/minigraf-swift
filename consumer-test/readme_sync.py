"""Check that the README quick start is what the consumer test runs.

Usage: python3 -I readme_sync.py README.md consumer-test/<quickstart file> [heading]

Takes the first fenced code block under the heading ("## Quick start" unless
given; a README covering two packages names each one's section). Every
non-blank line of it, with indentation trimmed, must appear in the consumer
file in the same order. The consumer may add lines around them (a package,
more imports, a test wrapper, assertions), but it cannot drop or reorder a
README line, so a README naming a symbol the package does not have fails CI.
Shared by every binding repo; the canonical copy is in
minigraf-binding-template.
"""

import sys


def quick_start_block(readme, heading):
    lines = readme.splitlines()
    try:
        start = next(i for i, l in enumerate(lines) if l.strip().lower() == heading.lower())
    except StopIteration:
        sys.exit(f"README has no '{heading}' section")
    block, inside = [], False
    for line in lines[start + 1:]:
        if line.startswith("## ") and not inside:
            break
        if line.lstrip().startswith("```"):
            if inside:
                return block
            inside = True
            continue
        if inside:
            block.append(line)
    sys.exit(f"'{heading}' has no complete fenced code block")


def main():
    readme_path, consumer_path = sys.argv[1], sys.argv[2]
    heading = sys.argv[3] if len(sys.argv) > 3 else "## Quick start"
    with open(readme_path, encoding="utf-8") as f:
        wanted = [l.strip() for l in quick_start_block(f.read(), heading) if l.strip()]
    with open(consumer_path, encoding="utf-8") as f:
        have = [l.strip() for l in f.read().splitlines()]
    pos = 0
    for line in wanted:
        try:
            pos = have.index(line, pos) + 1
        except ValueError:
            sys.exit(
                f"{consumer_path} does not run the README quick start: "
                f"no line {line!r} after line {pos}"
            )
    print(f"{readme_path} '{heading}' ({len(wanted)} lines) matches {consumer_path}")


if __name__ == "__main__":
    main()
