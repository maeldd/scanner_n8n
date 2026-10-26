#!/usr/bin/env python3
"""Convertit un export CSV Notion « Suivi de stock - pièces » en catalogue
local pour l'application.

Usage :
    python3 scripts/import-notion-csv.py "export-notion.csv" -o references.csv

Le fichier produit est volontairement simple (une ligne par référence) et
n'est pas destiné à être commité : c'est un fichier de données local. Il ne
contient volontairement ni stock ni alerte : ces informations restent dans
Notion.
"""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path

OUTPUT_COLUMNS = [
    "reference",
    "name",
    "category",
    "kind",
    "description",
    "qty_per_drone",
    "qty_per_rampe",
]


def as_int(value: str | None) -> int:
    value = (value or "").strip().replace("\u202f", "").replace(" ", "")
    if not value:
        return 0
    try:
        return int(float(value.replace(",", ".")))
    except ValueError:
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Export CSV Notion")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=Path("references.csv"),
        help="Fichier de sortie (défaut : references.csv)",
    )
    args = parser.parse_args()

    with args.source.open(encoding="utf-8-sig", newline="") as handle:
        rows = list(csv.DictReader(handle))

    if not rows:
        print("Le fichier source est vide.", file=sys.stderr)
        return 1

    cleaned: list[dict[str, object]] = []
    seen: set[str] = set()
    duplicates = 0

    for row in rows:
        reference = (row.get("Référence") or "").strip().upper()
        if not reference:
            continue
        if reference in seen:
            duplicates += 1
            continue
        seen.add(reference)

        cleaned.append(
            {
                "reference": reference,
                "name": (row.get("Nom") or "").strip(),
                "category": (row.get("Catégorie") or "").strip(),
                "kind": (row.get("Type") or "").strip(),
                "description": (row.get("Description") or "").strip(),
                "qty_per_drone": as_int(row.get("Quantité par drone")),
                "qty_per_rampe": as_int(row.get("Quantité par rampe")),
            }
        )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=OUTPUT_COLUMNS)
        writer.writeheader()
        writer.writerows(cleaned)

    print(f"{len(cleaned)} référence(s) écrite(s) dans {args.output}")
    if duplicates:
        print(f"{duplicates} doublon(s) ignoré(s).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
