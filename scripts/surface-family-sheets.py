#!/usr/bin/env python3
"""Build deterministic labelled and unlabelled orbital comparison sheets."""

from __future__ import annotations

import argparse
import html
import json
from pathlib import Path
from urllib.parse import quote

from PIL import Image, ImageDraw, ImageFont, ImageOps


BACKGROUND = (24, 27, 31)
FOREGROUND = (235, 237, 239)


def load_font(size: int) -> ImageFont.ImageFont:
    try:
        return ImageFont.truetype("arial.ttf", size)
    except OSError:
        return ImageFont.load_default(size=size)


def read_bodies(package: Path) -> list[dict]:
    manifest = json.loads((package / "manifest.json").read_text(encoding="utf-8"))
    bodies = manifest.get("bodies", [])
    if len(bodies) != 12:
        raise ValueError(f"expected 12 family bodies in manifest, found {len(bodies)}")
    for body in bodies:
        body["views"] = {scene["view"] for scene in body.get("scenes", [])}
        body["scene_records"] = {scene["view"]: scene for scene in body.get("scenes", [])}
        expected = f"{body['key']}/orbit/lit.png"
        image_path = package / expected
        if not image_path.is_file():
            raise FileNotFoundError(image_path)
        body["orbital_image"] = expected
    return bodies


def scene_metadata(body: dict, view: str) -> dict:
    return body["scene_records"].get(view, {}).get("metadata", {})


def scene_path(body: dict, view: str) -> str:
    record = body["scene_records"].get(view)
    return record.get("path", f"{body['key']}/{view}") if record else f"{body['key']}/{view}"


def tile_image(path: Path, width: int, height: int) -> Image.Image:
    with Image.open(path) as source:
        image = ImageOps.contain(source.convert("RGB"), (width, height), method=Image.Resampling.LANCZOS)
    tile = Image.new("RGB", (width, height), (9, 11, 13))
    tile.paste(image, ((width - image.width) // 2, (height - image.height) // 2))
    return tile


def make_sheet(package: Path, bodies: list[dict], output: Path, labelled: bool, tile_width: int,
               image_key: str | None = None, view: str = "orbit") -> list[dict]:
    columns = 4
    rows = 3
    label_height = 48 if labelled else 0
    tile_height = round(tile_width * 2 / 3)
    padding = 12
    sheet = Image.new(
        "RGB",
        (columns * tile_width + (columns + 1) * padding, rows * (tile_height + label_height) + (rows + 1) * padding),
        BACKGROUND,
    )
    draw = ImageDraw.Draw(sheet)
    font = load_font(18)
    small_font = load_font(14)
    mapping = []
    for index, body in enumerate(bodies):
        row, column = divmod(index, columns)
        x = padding + column * (tile_width + padding)
        y = padding + row * (tile_height + label_height + padding)
        relative_image = body["orbital_image"] if image_key is None else f"{scene_path(body, view)}/{image_key}"
        tile = tile_image(package / relative_image, tile_width, tile_height)
        sheet.paste(tile, (x, y))
        if labelled:
            family = body["family"].capitalize()
            seed = body["seed"]
            radius_km = body["radius_m"] / 1000.0
            draw.text((x + 4, y + tile_height + 4), f"{family} · seed {seed}", fill=FOREGROUND, font=font)
            draw.text((x + 4, y + tile_height + 27), f"{radius_km:g} km reference radius", fill=(175, 183, 192), font=small_font)
        mapping.append({
            "cell_index": index,
            "row": row,
            "column": column,
            "body_key": body["key"],
            "family": body["family"],
            "seed": body["seed"],
            "radius_m": body["radius_m"],
            "view": view,
            "image": relative_image,
            "metadata": scene_metadata(body, view),
        })
    sheet.save(output, format="PNG", optimize=True)
    return mapping


def make_diagnostic_sheet(package: Path, bodies: list[dict], output: Path, view: str, field: str, tile_width: int) -> None:
    columns = 4
    rows = (len(bodies) + columns - 1) // columns
    tile_height = round(tile_width * 2 / 3)
    label_height, padding = 42, 10
    sheet = Image.new("RGB", (columns * tile_width + (columns + 1) * padding,
        rows * (tile_height + label_height) + (rows + 1) * padding), BACKGROUND)
    draw = ImageDraw.Draw(sheet)
    font = load_font(16)
    for index, body in enumerate(bodies):
        row, column = divmod(index, columns)
        x = padding + column * (tile_width + padding)
        y = padding + row * (tile_height + label_height + padding)
        path = package / scene_path(body, view) / f"{field}.png"
        sheet.paste(tile_image(path, tile_width, tile_height), (x, y))
        draw.text((x + 4, y + tile_height + 5), f"{body['family']} · seed {body['seed']} · {body['radius_m']/1000:g} km", fill=FOREGROUND, font=font)
    sheet.save(output, format="PNG", optimize=True)


def make_material_sheet(package: Path, family_bodies: list[dict], output: Path, view: str, raw: bool, tile_width: int) -> None:
    channels = family_bodies[0]["definition"].get("material_channels", ["channel 1", "channel 2", "channel 3", "channel 4"])
    columns, rows = 4, len(family_bodies)
    tile_height = round(tile_width * 2 / 3)
    header_height, row_label, padding = 36, 24, 8
    width = row_label + columns * tile_width + (columns + 2) * padding
    height = header_height + rows * (tile_height + row_label) + (rows + 1) * padding
    sheet = Image.new("RGB", (width, height), BACKGROUND)
    draw = ImageDraw.Draw(sheet)
    header_font, row_font = load_font(15), load_font(13)
    for channel in range(4):
        x = row_label + padding + channel * (tile_width + padding)
        draw.text((x + 3, padding), channels[channel], fill=FOREGROUND, font=header_font)
    for row, body in enumerate(family_bodies):
        scene = package / scene_path(body, view)
        rgb_name = "raw_material.png" if raw else "material.png"
        fourth_name = "raw_material_4.png" if raw else "material_4.png"
        with Image.open(scene / rgb_name) as source:
            rgb_channels = source.convert("RGB").split()
            channel_images = [channel.convert("RGB") for channel in rgb_channels]
        with Image.open(scene / fourth_name) as source:
            channel_images.append(source.convert("RGB"))
        y = header_height + padding + row * (tile_height + row_label + padding)
        draw.text((padding, y + tile_height + 4), f"{body['seed']}", fill=(184, 192, 201), font=row_font)
        for channel, image in enumerate(channel_images):
            x = row_label + padding + channel * (tile_width + padding)
            fitted = ImageOps.contain(image, (tile_width, tile_height), method=Image.Resampling.LANCZOS)
            tile = Image.new("RGB", (tile_width, tile_height), (9, 11, 13))
            tile.paste(fitted, ((tile_width - fitted.width) // 2, (tile_height - fitted.height) // 2))
            sheet.paste(tile, (x, y))
    sheet.save(output, format="PNG", optimize=True)


def make_grid_sheet(package: Path, output: Path, rows: list[dict], columns: list[dict],
                    source_name, title: str, tile_width: int, audit: list[dict]) -> None:
    """Render a labelled comparison grid and add each source cell to the audit map."""
    tile_height = round(tile_width * 2 / 3)
    side_width, header_height, cell_label_height, padding = 142, 52, 22, 8
    width = side_width + len(columns) * tile_width + (len(columns) + 2) * padding
    height = header_height + len(rows) * (tile_height + cell_label_height) + (len(rows) + 1) * padding
    sheet = Image.new("RGB", (width, height), BACKGROUND)
    draw = ImageDraw.Draw(sheet)
    title_font, header_font, row_font = load_font(18), load_font(13), load_font(13)
    draw.text((padding, padding), title, fill=FOREGROUND, font=title_font)
    for column, column_info in enumerate(columns):
        x = side_width + padding + column * (tile_width + padding)
        draw.text((x + 3, padding + 27), column_info["label"], fill=(206, 213, 221), font=header_font)
    for row, row_info in enumerate(rows):
        y = header_height + padding + row * (tile_height + cell_label_height + padding)
        draw.text((padding, y + tile_height + 3), row_info["label"], fill=(206, 213, 221), font=row_font)
        for column, column_info in enumerate(columns):
            x = side_width + padding + column * (tile_width + padding)
            body = row_info["body"]
            view = column_info["view"].format(province=row_info.get("province", 0))
            relative = f"{scene_path(body, view)}/{source_name(body, view)}"
            image_path = package / relative
            if not image_path.is_file():
                raise FileNotFoundError(image_path)
            tile = tile_image(image_path, tile_width, tile_height)
            sheet.paste(tile, (x, y))
            audit.append({
                "sheet": output.name, "row": row, "column": column,
                "body_key": body["key"], "family": body["family"], "seed": body["seed"],
                "view": view, "image": relative,
                "metadata": scene_metadata(body, view),
            })
    sheet.save(output, format="PNG", optimize=True)


def find_province_selection(metadata: dict) -> dict | None:
    """Scene capture metadata may wrap fixture data; search its nested objects."""
    if isinstance(metadata, dict):
        selection = metadata.get("province_selection")
        if isinstance(selection, dict):
            return selection
        for value in metadata.values():
            found = find_province_selection(value) if isinstance(value, dict) else None
            if found:
                return found
    return None


def make_html(package: Path, generated: list[str], bodies: list[dict]) -> str:
    links = "".join(f"<li><a href='{quote(name)}'>{html.escape(name)}</a></li>" for name in generated)
    sections = []
    for body in bodies:
        scene_views = sorted(body["views"])
        for view in scene_views:
            path = scene_path(body, view)
            metadata = scene_metadata(body, view)
            selection = find_province_selection(metadata)
            selection_text = (f" · {html.escape(str(selection.get('province_name', '')))}" if selection else "")
            image_links = []
            scene_directory = package / path
            for image_path in sorted(scene_directory.glob("*.png")):
                relative = image_path.relative_to(package).as_posix()
                href = quote(relative, safe="/")
                image_links.append(f"<a href='{href}'>{html.escape(image_path.name)}</a>")
            metadata_json = html.escape(json.dumps(metadata, indent=2, ensure_ascii=False))
            details = f"<details><summary>Scene metadata</summary><pre>{metadata_json}</pre></details>"
            sections.append(
                f"<section><h3>{html.escape(body['family'])} · seed {body['seed']} · "
                f"{html.escape(body['key'])} / {html.escape(view)}{selection_text}</h3>"
                f"<p>{' · '.join(image_links) if image_links else 'No source images found'}</p>{details}</section>"
            )
    return ("<!doctype html><meta charset='utf-8'><title>Surface family sheets</title>"
            "<style>body{font:15px system-ui;background:#17191c;color:#eee;margin:24px}a{color:#b8d5ff}"
            "li{margin:6px}section{border-top:1px solid #444;padding:8px 0}pre{white-space:pre-wrap;"
            "background:#222;padding:12px;max-height:32em;overflow:auto}</style>"
            "<h1>Surface family comparison sheets</h1><h2>Generated sheets</h2><ul>" + links +
            "</ul><h2>Source scenes and metadata</h2>" + "".join(sections))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="surface_family_reference output directory")
    parser.add_argument("--tile-width", type=int, default=360)
    args = parser.parse_args()
    package = args.package.resolve()
    if args.tile_width < 128 or args.tile_width > 1200:
        parser.error("--tile-width must be in 128..=1200")
    bodies = read_bodies(package)
    labelled = package / "orbital-contact-labelled.png"
    unlabelled = package / "orbital-contact-unlabelled.png"
    mapping = make_sheet(package, bodies, labelled, True, args.tile_width)
    make_sheet(package, bodies, unlabelled, False, args.tile_width)
    (package / "orbital-contact-mapping.json").write_text(
        json.dumps({"sheet_order": "manifest bodies order, row-major 4 columns by 3 rows", "cells": mapping}, indent=2) + "\n",
        encoding="utf-8",
    )
    generated = [labelled.name, unlabelled.name]
    audit = []
    directed = bool(json.loads((package / "manifest.json").read_text(encoding="utf-8")).get("geological_director_enabled"))
    geometry_available = all((package / scene_path(body, "orbit") / "geometry.png").is_file() for body in bodies)
    if geometry_available:
        for is_labelled in (False, True):
            name = f"orbital-geometry-{'labelled' if is_labelled else 'unlabelled'}.png"
            path = package / name
            mapping = make_sheet(package, bodies, path, is_labelled, args.tile_width,
                                 image_key="geometry.png", view="orbit")
            generated.append(name)
            audit.extend({"sheet": name, **cell} for cell in mapping)
    elif directed:
        raise FileNotFoundError("director package is missing one or more orbit/geometry.png images")
    for view in ("orbit", "regional", "near", "regional-landmark", "near-landmark"):
        view_bodies = [body for body in bodies if view in body["views"]]
        if not view_bodies:
            continue
        for field in ("height", "shape", "normal", "analytic_normal"):
            path = package / f"diagnostic-{view}-{field}.png"
            make_diagnostic_sheet(package, view_bodies, path, view, field, min(args.tile_width, 300))
            generated.append(path.name)
        for family in ("rocky", "icy", "volcanic"):
            family_bodies = [
                body for body in view_bodies if body["family"] == family
            ]
            if not family_bodies:
                continue
            for raw in (False, True):
                path = package / f"material-{family}-{view}-{'raw-' if raw else ''}weights.png"
                make_material_sheet(package, family_bodies, path, view, raw, min(args.tile_width, 260))
                generated.append(path.name)
    if directed:
        families = ("rocky", "icy", "volcanic")
        province_bodies = {
            family: next((body for body in bodies if body["family"] == family and body["seed"] == 0
                          and all(f"province-{province}-scale-0" in body["views"] for province in range(4))), None)
            for family in families
        }
        missing_families = [family for family, body in province_bodies.items() if body is None]
        if missing_families:
            raise ValueError(f"director package lacks seed-0 province captures for: {', '.join(missing_families)}")
        hierarchical = bool(json.loads((package / "manifest.json").read_text(encoding="utf-8")).get("hierarchical_detail_enabled"))
        scales = ["20 km", "2 km", "256 m", "32 m"] + (["8 m"] if hierarchical else [])
        for family, body in province_bodies.items():
            for image_name, field_label in (("geometry.png", "geometry"), ("lit.png", "lit"),
                                            ("normal.png", "normal"), ("material.png", "material")):
                columns = [{"view": f"province-{{province}}-scale-{scale}", "label": scales[scale]}
                           for scale in range(len(scales))]
                rows = [{"body": body, "province": province, "label": (find_province_selection(scene_metadata(body, f"province-{province}-scale-0")) or {}).get("province_name", f"Province {province}")}
                        for province in range(4)]
                name = f"province-{family}-{field_label}.png"
                path = package / name
                make_grid_sheet(package, path, rows, columns, lambda _body, _view, n=image_name: n,
                                f"{family.capitalize()} provinces · {field_label}", min(args.tile_width, 240), audit)
                generated.append(name)
        comparison_scales = range(2, len(scales)) if hierarchical else range(len(scales))
        for scale in comparison_scales:
            scale_label = scales[scale]
            rows = [{"body": province_bodies[family], "label": family.capitalize()} for family in families]
            columns = [{"view": f"province-{province}-scale-{scale}", "label": f"Province {province}"}
                       for province in range(4)]
            name = f"province-cross-family-scale-{scale}-geometry.png"
            path = package / name
            make_grid_sheet(package, path, rows, columns, lambda _body, _view: "geometry.png",
                            f"Cross-family geometry · {scale_label}", min(args.tile_width, 240), audit)
            generated.append(name)
        if hierarchical:
            unbiased_rows = [{"body": province_bodies[family], "label": family.capitalize()}
                             for family in families]
            unbiased_columns = [
                {"view": "unbiased-32m", "label": "Unbiased 32 m"},
                {"view": "unbiased-8m", "label": "Unbiased 8 m"},
            ]
            name = "unbiased-cross-family-geometry.png"
            path = package / name
            make_grid_sheet(package, path, unbiased_rows, unbiased_columns,
                            lambda _body, _view: "geometry.png",
                            "Unbiased cross-family geometry · fixed +Z anchor",
                            min(args.tile_width, 280), audit)
            generated.append(name)
        director_channels = ["director-age", "director-activity", "director-resurfacing",
                             "director-impact-retention", "director-relief-potential",
                             *(f"province-{index}" for index in range(4)),
                             *(f"process-{index}" for index in range(4))]
        for channel in director_channels:
            available = [body for body in bodies if (package / scene_path(body, "orbit") / f"{channel}.png").is_file()]
            if available:
                path = package / f"{channel}-orbital.png"
                make_diagnostic_sheet(package, available, path, "orbit", channel, min(args.tile_width, 280))
                generated.append(path.name)
                for index, body in enumerate(available):
                    audit.append({
                        "sheet": path.name, "row": index // 4, "column": index % 4,
                        "body_key": body["key"], "family": body["family"], "seed": body["seed"],
                        "view": "orbit", "field": channel,
                        "image": f"{scene_path(body, 'orbit')}/{channel}.png",
                        "metadata": scene_metadata(body, "orbit"),
                    })
    (package / "sheets-audit-mapping.json").write_text(
        json.dumps({"sheet_order": generated, "cells": audit}, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    (package / "sheets.html").write_text(make_html(package, generated, bodies), encoding="utf-8")
    index_path = package / "index.html"
    if index_path.is_file():
        index = index_path.read_text(encoding="utf-8")
        link = "<p><a href='sheets.html'>Open labelled, unlabelled, geometry and four-channel material comparison sheets</a></p>"
        if "sheets.html" not in index:
            index_path.write_text(index.replace("<h1>", link + "<h1>", 1), encoding="utf-8")
    print(f"wrote {labelled}")
    print(f"wrote {unlabelled}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
