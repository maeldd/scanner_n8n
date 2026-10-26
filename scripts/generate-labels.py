#!/usr/bin/env python3
"""Génère une planche HTML imprimable d'étiquettes QR (IN/OUT) à partir de
`references.csv`.

Chaque étiquette contient deux QR codes :
    IN|REF   -> entrée en stock
    OUT|REF  -> sortie de stock

Le HTML est autonome (SVG inline), sans accès réseau. Ouvrez-le dans un
navigateur puis imprimez-le sur A4 (8 étiquettes par page, 2 colonnes x 4
lignes), ou enregistrez-le en PDF.

Dépendance :
    python3 -m venv .venv
    .venv/bin/pip install segno
    .venv/bin/python scripts/generate-labels.py

Exemples :
    .venv/bin/python scripts/generate-labels.py
    .venv/bin/python scripts/generate-labels.py --only DCO001 DA001
    .venv/bin/python scripts/generate-labels.py -o /tmp/etiquettes.html
"""

from __future__ import annotations

import argparse
import csv
import html
import re
import sys
from pathlib import Path

try:
    import segno
except ImportError:  # pragma: no cover - message utilisateur
    print(
        "Le module 'segno' est requis. Installez-le avec :\n"
        "  python3 -m venv .venv && .venv/bin/pip install segno",
        file=sys.stderr,
    )
    raise SystemExit(1)

LABELS_PER_PAGE = 8

CSS = """
@page { size: A4; margin: 8mm; }
* { box-sizing: border-box; }
body {
  margin: 0;
  background: #e2e8f0;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Arial, sans-serif;
  color: #0f172a;
}
.page {
  width: 194mm;
  height: 281mm;
  margin: 0 auto 8mm;
  padding: 0;
  display: grid;
  grid-template-columns: 1fr 1fr;
  grid-template-rows: repeat(4, 1fr);
  gap: 3mm;
  page-break-after: always;
  break-after: page;
}
.page:last-child { page-break-after: auto; break-after: auto; }
.label {
  border: 0.45mm solid #94a3b8;
  border-radius: 2mm;
  padding: 2.5mm 3mm;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: #fff;
}
.label-header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 2mm;
  padding-bottom: 1.5mm;
  border-bottom: 0.3mm solid #cbd5e1;
}
.ref {
  font-family: "SF Mono", Menlo, Consolas, monospace;
  font-size: 5.2mm;
  font-weight: 800;
  letter-spacing: 0.2mm;
  white-space: nowrap;
}
.category {
  font-size: 2.5mm;
  color: #64748b;
  text-transform: uppercase;
  letter-spacing: 0.2mm;
  white-space: nowrap;
}
.name {
  font-size: 3.1mm;
  font-weight: 600;
  line-height: 1.15;
  margin: 1.4mm 0 0.6mm;
  height: 6.5mm;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.description {
  font-size: 2.4mm;
  color: #475569;
  line-height: 1.15;
  height: 5mm;
  margin-bottom: 1mm;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.qr-row {
  flex: 1;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 2mm;
  align-items: center;
}
.qr-box {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.8mm;
  padding: 1mm;
  border-radius: 1.5mm;
}
.qr-box.in { background: #fff; border: 0.35mm solid #6ee7b7; }
.qr-box.out { background: #fff; border: 0.35mm solid #fca5a5; }
.qr-box svg { width: 27mm; height: 27mm; display: block; }
.qr-action {
  font-size: 3.4mm;
  font-weight: 800;
  letter-spacing: 0.5mm;
}
.qr-box.in .qr-action { color: #047857; }
.qr-box.out .qr-action { color: #b91c1c; }
.qr-code-text {
  font-family: "SF Mono", Menlo, Consolas, monospace;
  font-size: 2.2mm;
  color: #475569;
}
.label-footer {
  display: flex;
  justify-content: space-between;
  gap: 2mm;
  margin-top: 1mm;
  font-size: 2.3mm;
  color: #64748b;
}
.instructions {
  max-width: 194mm;
  margin: 0 auto 6mm;
  padding: 4mm 5mm;
  background: #fff;
  border-left: 1.2mm solid #4f46e5;
  font-size: 3.2mm;
  line-height: 1.4;
}
@media print {
  body { background: #fff; }
  .instructions { display: none; }
  .page { margin: 0; }
}
"""


def qr_svg(value: str) -> str:
    qr = segno.make(value, error="m")
    svg = qr.svg_inline(scale=1, border=3, dark="#0f172a", light="#ffffff")
    # segno's inline SVG has width/height but no viewBox. Without a viewBox,
    # CSS resizing enlarges the viewport without scaling the modules; the QR
    # would appear tiny in the corner. Replace the fixed size by a viewBox and
    # let CSS choose the printed size.
    match = re.search(r'<svg width="(\d+)" height="(\d+)"', svg)
    if match:
        width, height = match.group(1), match.group(2)
        svg = svg.replace(
            match.group(0),
            f'<svg viewBox="0 0 {width} {height}"',
            1,
        )
    return svg


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "catalog",
        nargs="?",
        type=Path,
        default=Path("references.csv"),
        help="Catalogue CSV (défaut : references.csv)",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=Path("labels.html"),
        help="Fichier HTML de sortie (défaut : labels.html)",
    )
    parser.add_argument(
        "--only",
        nargs="+",
        metavar="REF",
        help="Ne générer que ces références (utile pour une réimpression)",
    )
    args = parser.parse_args()

    if not args.catalog.exists():
        print(
            f"Catalogue introuvable : {args.catalog}\n"
            "Générez-le avec scripts/import-notion-csv.py.",
            file=sys.stderr,
        )
        return 1

    with args.catalog.open(encoding="utf-8-sig", newline="") as handle:
        rows = list(csv.DictReader(handle))

    wanted = {ref.strip().upper() for ref in args.only} if args.only else None

    labels: list[dict[str, str]] = []
    seen: set[str] = set()
    for row in rows:
        reference = (row.get("reference") or "").strip().upper()
        if not reference or reference in seen:
            continue
        if wanted is not None and reference not in wanted:
            continue
        seen.add(reference)
        labels.append(
            {
                "reference": reference,
                "name": (row.get("name") or "").strip() or reference,
                "description": (row.get("description") or "").strip(),
                "category": (row.get("category") or "").strip(),
                "kind": (row.get("kind") or "").strip(),
            }
        )

    if not labels:
        print("Aucune référence à générer.", file=sys.stderr)
        return 1

    pages: list[str] = []
    for page_start in range(0, len(labels), LABELS_PER_PAGE):
        page_labels = labels[page_start : page_start + LABELS_PER_PAGE]
        cards: list[str] = []
        for label in page_labels:
            reference = html.escape(label["reference"])
            name = html.escape(label["name"])
            description = html.escape(label["description"])
            category = html.escape(label["category"] or label["kind"] or "")
            cards.append(
                f"""
      <div class="label">
        <div class="label-header">
          <span class="ref">{reference}</span>
          <span class="category">{category}</span>
        </div>
        <div class="name">{name}</div>
        <div class="description">{description}</div>
        <div class="qr-row">
          <div class="qr-box in">
            {qr_svg(f"IN|{label['reference']}")}
            <span class="qr-action">ENTRÉE</span>
            <span class="qr-code-text">IN|{reference}</span>
          </div>
          <div class="qr-box out">
            {qr_svg(f"OUT|{label['reference']}")}
            <span class="qr-action">SORTIE</span>
            <span class="qr-code-text">OUT|{reference}</span>
          </div>
        </div>
        <div class="label-footer">
          <span>1 scan = 1 pièce</span>
          <span>Scanner Pièces</span>
        </div>
      </div>"""
            )
        pages.append(f'  <section class="page">{"".join(cards)}</section>')

    document = f"""<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<title>Étiquettes QR — Scanner Pièces</title>
<style>{CSS}</style>
</head>
<body>
<div class="instructions">
  <strong>Impression :</strong> A4, échelle 100 %, marges par défaut. 8 étiquettes par page
  (2 colonnes × 4 lignes). Découper sur les traits. Chaque couple de QR codes est propre
  à une référence : <code>IN|REF</code> pour une entrée, <code>OUT|REF</code> pour une sortie.
  Les QR ne contiennent pas de quantité.
</div>
{''.join(pages)}
</body>
</html>
"""

    args.output.write_text(document, encoding="utf-8")
    print(f"{len(labels)} étiquette(s) générée(s) dans {args.output}")
    print(f"Pages A4 : {(len(labels) + LABELS_PER_PAGE - 1) // LABELS_PER_PAGE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
