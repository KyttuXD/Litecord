#!/usr/bin/env python3
"""Compare two directories of Litecord UI screenshots (see docs/UI_PARITY.md).

    python3 scripts/ui_diff.py out-linux out-macos --out ui-diff

Both directories come from `litecord --screens <dir>`. For every PNG that exists on both
sides the script measures the share of pixels that differ, writes a diff image (the first
picture dimmed, differing pixels in red) and adds a row to report.md. A PNG that exists on
one side only is listed as missing and makes the exit status 1. Plain differences never
fail the run unless --fail-above is given.

Needs Pillow (pip install pillow).
"""

import argparse
import os
import sys

from PIL import Image, ImageChops


def list_pngs(directory):
    return sorted(
        name for name in os.listdir(directory)
        if name.lower().endswith(".png") and os.path.isfile(os.path.join(directory, name))
    )


def compare(path_a, path_b, tolerance):
    """Returns (ratio, diff_image, note). ratio is the share of differing pixels."""
    a = Image.open(path_a).convert("RGB")
    b = Image.open(path_b).convert("RGB")
    if a.size != b.size:
        return 1.0, None, "size differs: %dx%d vs %dx%d" % (a.size + b.size)

    delta = ImageChops.difference(a, b)
    r, g, bl = delta.split()
    strongest = ImageChops.lighter(ImageChops.lighter(r, g), bl)
    mask = strongest.point(lambda value: 255 if value > tolerance else 0)
    changed = mask.histogram()[255]
    ratio = changed / float(a.size[0] * a.size[1])

    if changed == 0:
        return 0.0, None, ""

    faded = Image.blend(a, Image.new("RGB", a.size, (0, 0, 0)), 0.65)
    red = Image.new("RGB", a.size, (255, 0, 0))
    diff_image = Image.composite(red, faded, mask)
    return ratio, diff_image, ""


def main():
    parser = argparse.ArgumentParser(description="Compare two directories of UI screenshots.")
    parser.add_argument("dir_a", help="first directory (for example out-linux)")
    parser.add_argument("dir_b", help="second directory (for example out-macos)")
    parser.add_argument("--out", default="ui-diff", help="output directory (default: ui-diff)")
    parser.add_argument("--label-a", default=None, help="name of the first side in the report")
    parser.add_argument("--label-b", default=None, help="name of the second side in the report")
    parser.add_argument("--tolerance", type=int, default=8,
                        help="largest per-channel difference (0-255) still counted as equal (default: 8)")
    parser.add_argument("--fail-above", type=float, default=None, metavar="PERCENT",
                        help="also exit 1 when a screen differs by more than this percentage")
    args = parser.parse_args()

    for directory in (args.dir_a, args.dir_b):
        if not os.path.isdir(directory):
            print("not a directory: %s" % directory, file=sys.stderr)
            return 2

    label_a = args.label_a or os.path.basename(os.path.normpath(args.dir_a))
    label_b = args.label_b or os.path.basename(os.path.normpath(args.dir_b))
    os.makedirs(args.out, exist_ok=True)

    names_a = list_pngs(args.dir_a)
    names_b = list_pngs(args.dir_b)
    set_a, set_b = set(names_a), set(names_b)
    all_names = sorted(set_a | set_b)

    rows = []
    missing = 0
    worst_percent = 0.0
    identical = 0
    for name in all_names:
        if name not in set_b:
            rows.append((name, "only in %s" % label_a, "", ""))
            missing += 1
            continue
        if name not in set_a:
            rows.append((name, "only in %s" % label_b, "", ""))
            missing += 1
            continue

        ratio, diff_image, note = compare(
            os.path.join(args.dir_a, name), os.path.join(args.dir_b, name), args.tolerance)
        percent = ratio * 100.0
        worst_percent = max(worst_percent, percent)
        link = ""
        if diff_image is not None:
            diff_name = "diff-" + name
            diff_image.save(os.path.join(args.out, diff_name))
            link = "[%s](%s)" % (diff_name, diff_name)
        if ratio == 0.0:
            identical += 1
            status = "identical"
        else:
            status = "differs"
        rows.append((name, status, "%.3f%%" % percent, link or note))
        print("%-48s %-10s %8.3f%%  %s" % (name, status, percent, note))

    lines = []
    lines.append("# UI parity: %s vs %s" % (label_a, label_b))
    lines.append("")
    lines.append("- Screens in %s: %d" % (label_a, len(names_a)))
    lines.append("- Screens in %s: %d" % (label_b, len(names_b)))
    lines.append("- Identical: %d" % identical)
    lines.append("- Different: %d" % (len(all_names) - missing - identical))
    lines.append("- Present on one side only: %d" % missing)
    lines.append("- Largest difference: %.3f%% of the pixels" % worst_percent)
    lines.append("- Pixels count as different above a per-channel step of %d" % args.tolerance)
    lines.append("")
    lines.append("| Screen | Result | Different pixels | Diff image |")
    lines.append("| --- | --- | ---: | --- |")
    for name, status, percent, extra in rows:
        lines.append("| %s | %s | %s | %s |" % (name, status, percent, extra))
    lines.append("")

    report_path = os.path.join(args.out, "report.md")
    with open(report_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines))
    print("report: %s" % report_path)

    if missing:
        print("%d screen(s) exist on one side only" % missing, file=sys.stderr)
        return 1
    if args.fail_above is not None and worst_percent > args.fail_above:
        print("a screen differs by %.3f%%, above %.3f%%" % (worst_percent, args.fail_above),
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
