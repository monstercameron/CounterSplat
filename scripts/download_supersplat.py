#!/usr/bin/env python3
"""Download public SuperSplat viewer assets, including every published LOD, for local development.

Downloads only URLs actually named by the public viewer bootstrap and its metadata. Partial files
are staged separately, verified, and renamed after success. Rerun the same command to resume.
"""
import argparse
import concurrent.futures
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import sys
import time
from urllib.error import HTTPError
from urllib.parse import quote, urljoin, urlparse
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "assets" / "levels"
USER_AGENT = "CounterSplat-development/1.0"
MIN_FREE = 2 * 1024 * 1024 * 1024
SCENES = {
    "9f0f903a": ("La Cité - Angers", "jeromebg", None),
    "58d333ae": ("The Barn", "dongiku", None),
    "541bac0c": ("Botanical Garden Greenhouses - Kiel", "simonbethke", "CC BY 4.0"),
    "947dbaec": ("Falls", "vice", None),
    "b8262a29": ("260204_서울시 용산구 우사단로4다길 14(14, Usadan-ro 4da-gil, Yongsan-gu)", "mias", None),
    "ff1a8800": ("Ananuri Fortress", "maorio", None),
    "9649b9e2": ("서울특별시 은평구 불광동 264-14(264-14, Bulgwang-dong, Eunpyeong-gu, Seoul)", "mias", None),
    "fd3083e4": ("San Juan", "AJ Creek / virtualworldtours", None),
    "9cbaba42": ("Fort Clatsop", "virtualworldtours", None),
    "b0ff3cb9": ("El Romeral", "getarq", None),
    "fb3b5ed5": ("Nelson Ghost Town", "tosolini", "CC BY 4.0"),
}
# Finish smaller scenes before the 123-million-splat greenhouse.
ORDER = ["58d333ae", "9f0f903a", "947dbaec", "b8262a29", "ff1a8800", "9649b9e2", "541bac0c",
         "fb3b5ed5", "b0ff3cb9", "9cbaba42", "fd3083e4"]


def relative_path(value):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value or value.startswith("/"):
        raise ValueError(f"unsafe metadata path: {value!r}")
    if any(part in ("", ".", "..") for part in value.split("/")):
        raise ValueError(f"unsafe metadata path: {value!r}")
    if any(ord(char) < 32 for char in value):
        raise ValueError("control character in metadata path")
    return PurePosixPath(value)


def target_path(root, relative):
    relative = relative_path(str(relative))
    target = root.joinpath(*relative.parts)
    if not target.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"metadata path leaves scene directory: {relative}")
    return target


def request(url):
    return urlopen(Request(url, headers={"User-Agent": USER_AGENT}), timeout=60)


def small_response(url, limit=16 * 1024 * 1024):
    with request(url) as response:
        encoded = response.read(limit + 1)
        if len(encoded) > limit:
            raise ValueError(f"metadata exceeds {limit} bytes: {url}")
        if response.headers.get("Content-Encoding", "").lower() == "gzip":
            encoded = gzip.decompress(encoded)
        if len(encoded) > limit:
            raise ValueError(f"decoded metadata exceeds {limit} bytes: {url}")
        return encoded


def write_atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".part")
    with temporary.open("wb") as output:
        output.write(data)
    os.replace(temporary, path)


def webp_valid(path):
    with path.open("rb") as source:
        header = source.read(12)
    return (len(header) == 12 and header[:4] == b"RIFF" and header[8:12] == b"WEBP"
            and struct.unpack("<I", header[4:8])[0] + 8 == path.stat().st_size)


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def cached_valid(path):
    if not path.is_file() or path.stat().st_size == 0:
        return False
    try:
        if path.suffix.lower() == ".webp":
            return webp_valid(path)
        if path.suffix.lower() == ".json":
            json.loads(path.read_bytes())
        return True
    except (OSError, ValueError):
        return False


def fetch_file(job):
    scene_id, url, path = job
    path.parent.mkdir(parents=True, exist_ok=True)
    if cached_valid(path):
        return scene_id, path, path.stat().st_size, "cached"
    last = None
    for attempt in range(5):
        temporary = path.with_name(path.name + ".part")
        try:
            if shutil.disk_usage(path.parent).free < MIN_FREE:
                raise OSError("stopping to preserve at least 2 GiB of free disk space")
            with request(url) as response, temporary.open("wb") as output:
                encoding = response.headers.get("Content-Encoding", "").lower()
                declared = response.headers.get("Content-Length")
                source = gzip.GzipFile(fileobj=response) if encoding == "gzip" else response
                count = 0
                while block := source.read(1024 * 1024):
                    count += len(block)
                    output.write(block)
                if encoding != "gzip" and declared is not None and count != int(declared):
                    raise IOError(f"incomplete response: {count} of {declared} bytes")
            if not cached_valid(temporary):
                # A staged .part file has no useful suffix; validate the intended format explicitly.
                if not temporary.is_file() or temporary.stat().st_size == 0:
                    raise IOError("empty response")
            if path.suffix.lower() == ".webp" and not webp_valid(temporary):
                raise IOError("invalid or incomplete WebP texture")
            if path.suffix.lower() == ".json":
                json.loads(temporary.read_bytes())
            os.replace(temporary, path)
            return scene_id, path, path.stat().st_size, "downloaded"
        except (OSError, ValueError, EOFError, HTTPError) as error:
            last = error
            if temporary.exists():
                temporary.unlink()
            if isinstance(error, HTTPError) and error.code in (401, 403, 404):
                break
            if "preserve at least 2 GiB" in str(error):
                break
            if attempt < 4:
                time.sleep(min(2 ** attempt, 12))
    raise RuntimeError(f"{scene_id} {url}: {last}")


def metadata_files(meta):
    result = []
    def visit(node):
        if isinstance(node, dict):
            for key, value in node.items():
                if key == "files":
                    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
                        raise ValueError("invalid SOG texture file list")
                    result.extend(value)
                else:
                    visit(value)
        elif isinstance(node, list):
            for item in node:
                visit(item)
    visit(meta)
    if not result:
        raise ValueError("SOG metadata contains no texture file references")
    return list(dict.fromkeys(result))


def scene_info(scene_id):
    html = small_response("https://superspl.at/s?id=" + scene_id).decode("utf-8")
    match = re.search(r'<script type="application/json" id="sse-bootstrap">(.*?)</script>', html, re.S)
    if not match:
        raise ValueError("viewer bootstrap missing")
    bootstrap = json.loads(match.group(1))
    content = bootstrap.get("contentUrl", "")
    parsed = urlparse(content)
    if parsed.scheme != "https" or parsed.netloc != "d28zzqy0iyovbz.cloudfront.net":
        raise ValueError("viewer content URL is not the expected public SuperSplat CDN")
    base = content.rsplit("/", 1)[0] + "/"
    if not parsed.path.startswith("/" + scene_id + "/"):
        raise ValueError("viewer content URL belongs to a different scene")
    data = small_response(content)
    manifest = json.loads(data)
    root = DESTINATION / scene_id
    root.mkdir(parents=True, exist_ok=True)
    content_name = relative_path(parsed.path.rsplit("/", 1)[-1])
    write_atomic(target_path(root, content_name), data)
    if isinstance(bootstrap.get("settings"), dict):
        write_atomic(root / "viewer-settings.json", json.dumps(bootstrap["settings"], indent=2).encode() + b"\n")
    title, author, license_name = SCENES[scene_id]
    info = {
        "scene_id": scene_id, "title": title, "creator": author,
        "scene_url": "https://superspl.at/scene/" + scene_id,
        "viewer_content_url": content, "license_shown_on_scene_page": license_name,
        "format": "streamed_sog" if "filenames" in manifest else "single_sog",
        "lod_levels": manifest.get("lodLevels"), "lod_counts": manifest.get("counts"),
        "single_sog_count": manifest.get("count") if "filenames" not in manifest else None,
    }
    write_atomic(root / "source.json", json.dumps(info, indent=2, ensure_ascii=False).encode() + b"\n")
    if "filenames" in manifest:
        names = manifest["filenames"]
        if not isinstance(names, list) or not names:
            raise ValueError("empty or invalid LOD metadata filename list")
        metadata = [relative_path(name) for name in names]
        if len(metadata) != len(set(metadata)):
            raise ValueError("duplicate chunk metadata references")
    else:
        metadata = [content_name]
    jobs = [(scene_id, urljoin(base, quote(str(name), safe="/")), target_path(root, name)) for name in metadata]
    collision = bootstrap.get("collisionUrl")
    collision_jobs = []
    if collision:
        parsed_collision = urlparse(collision)
        if parsed_collision.scheme != "https" or parsed_collision.netloc != parsed.netloc or not collision.startswith(base):
            raise ValueError("collision URL leaves the scene CDN directory")
        if not collision.endswith("scene.voxel.json"):
            raise ValueError("unexpected collision metadata filename")
        collision_jobs = [(scene_id, collision, root / "scene.voxel.json"),
                          (scene_id, collision[:-5] + ".bin", root / "scene.voxel.bin")]
    return {"id": scene_id, "root": root, "base": base, "metadata": jobs,
            "collision": collision_jobs, "expected": {target_path(root, content_name)}, "errors": [], "info": info}


def inspect_lod_tree(root):
    """Verify every LOD range against its chunk metadata and the manifest's per-level counts."""
    manifest_path = root / "lod-meta.json"
    if not manifest_path.is_file():
        return None
    manifest = json.loads(manifest_path.read_bytes())
    names = manifest["filenames"]
    counts = manifest["counts"]
    if manifest["lodLevels"] != len(counts):
        raise ValueError("LOD level count disagrees with count array")
    chunk_counts = [json.loads(target_path(root, relative_path(name)).read_bytes())["count"] for name in names]
    totals = [0] * len(counts)
    leaves = 0
    pending = [manifest["tree"]]
    while pending:
        node = pending.pop()
        if node.get("children"):
            pending.extend(node["children"])
            continue
        leaves += 1
        for key, segment in node.get("lods", {}).items():
            level = int(key)
            index, offset, count = segment["file"], segment["offset"], segment["count"]
            if not (0 <= level < len(counts) and 0 <= index < len(chunk_counts)
                    and offset >= 0 and count >= 0 and offset + count <= chunk_counts[index]):
                raise ValueError(f"invalid LOD range in leaf {leaves}, level {key}")
            totals[level] += count
    if totals != counts:
        raise ValueError("LOD tree totals disagree with manifest counts")
    return {"levels": len(counts), "chunk_metadata_files": len(names), "leaves": leaves, "counts": counts}


def run_jobs(label, jobs, workers):
    print(f"{label}: {len(jobs)} files", flush=True)
    results = []
    failures = []
    last_report = time.monotonic()
    completed_bytes = 0
    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(fetch_file, job): job for job in jobs}
        for done, future in enumerate(concurrent.futures.as_completed(futures), 1):
            job = futures[future]
            try:
                result = future.result()
                results.append(result)
                completed_bytes += result[2]
            except Exception as error:
                failures.append((job[0], str(error)))
                print(f"FAILED {error}", flush=True)
            now = time.monotonic()
            if done == len(jobs) or now - last_report >= 5:
                print(f"{label}: {done}/{len(jobs)} files, {completed_bytes / 1024**2:.1f} MiB handled, "
                      f"{len(failures)} errors, {now - started:.0f}s", flush=True)
                last_report = now
    return results, failures


def verify_local(scene_id):
    """Verify the saved inventory, metadata references, checksums and all LOD ranges offline."""
    root = DESTINATION / scene_id
    inventory = json.loads((root / "download-inventory.json").read_bytes())
    if not inventory.get("complete") or inventory.get("errors"):
        raise ValueError("the recorded download is incomplete")
    entries = {entry["path"]: entry for entry in inventory["files"]}
    if len(entries) != len(inventory["files"]):
        raise ValueError("the inventory contains duplicate paths")
    manifest_name = "lod-meta.json" if (root / "lod-meta.json").is_file() else "meta.json"
    manifest = json.loads((root / manifest_name).read_bytes())
    meta_names = manifest["filenames"] if manifest_name == "lod-meta.json" else ["meta.json"]
    expected = {manifest_name}
    for name in meta_names:
        rel = relative_path(name)
        expected.add(str(rel))
        meta = json.loads(target_path(root, rel).read_bytes())
        for filename in metadata_files(meta):
            expected.add(str(relative_path(str(PurePosixPath(rel.parent) / relative_path(filename)))))
    if (root / "scene.voxel.json").exists() or (root / "scene.voxel.bin").exists():
        expected.update(["scene.voxel.json", "scene.voxel.bin"])
    if expected != set(entries):
        raise ValueError("inventory paths do not match scene metadata references")
    for name in sorted(expected):
        path = target_path(root, relative_path(name))
        record = entries[name]
        if not cached_valid(path) or path.stat().st_size != record["bytes"] or file_digest(path) != record["sha256"]:
            raise ValueError(f"integrity failure: {name}")
    lod = inspect_lod_tree(root)
    if (root / "scene.voxel.json").exists():
        meta = json.loads((root / "scene.voxel.json").read_bytes())
        if (root / "scene.voxel.bin").stat().st_size != (meta["nodeCount"] + meta["leafDataCount"]) * 4:
            raise ValueError("collision binary does not match its metadata")
    return len(expected), lod


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ids", nargs="*", metavar="SCENE_ID", help="scene IDs; default: all listed scenes")
    parser.add_argument("--workers", type=int, default=12, help="parallel downloads (1–16, default 12)")
    parser.add_argument("--verify-only", action="store_true", help="verify existing downloads without network access")
    args = parser.parse_args()
    if not 1 <= args.workers <= 16:
        parser.error("workers must be between 1 and 16")
    unknown = [scene_id for scene_id in args.ids if scene_id not in SCENES]
    if unknown:
        parser.error("unknown scene ID(s): " + ", ".join(unknown))
    chosen = list(dict.fromkeys(args.ids or ORDER))
    if args.verify_only:
        failed = False
        for scene_id in chosen:
            try:
                count, lod = verify_local(scene_id)
                detail = f"{lod['levels']} LOD levels" if lod else "single SOG; no separate LOD manifest"
                print(f"{scene_id}: VERIFIED | {count} files | {detail}", flush=True)
            except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
                failed = True
                print(f"{scene_id}: FAILED | {error}", flush=True)
        return int(failed)
    contexts = {}
    for scene_id in chosen:
        try:
            context = scene_info(scene_id)
            contexts[scene_id] = context
            for _, _, path in context["metadata"] + context["collision"]:
                context["expected"].add(path)
            print(f"{scene_id}: {context['info']['title']} | {context['info']['format']} | "
                  f"{context['info']['lod_levels'] or 0} published LOD levels | {len(context['metadata'])} metadata files", flush=True)
        except Exception as error:
            print(f"FAILED {scene_id}: {error}", flush=True)
    metadata_jobs = [job for scene_id in chosen if scene_id in contexts for job in contexts[scene_id]["metadata"]]
    metadata_results, metadata_errors = run_jobs("Chunk metadata", metadata_jobs, args.workers)
    for scene_id, error in metadata_errors:
        contexts[scene_id]["errors"].append(error)
    texture_jobs = []
    seen = set()
    for scene_id, meta_path, _, _ in metadata_results:
        context = contexts[scene_id]
        try:
            meta = json.loads(meta_path.read_bytes())
            relative_meta = meta_path.relative_to(context["root"])
            for name in metadata_files(meta):
                relative = relative_path(str(PurePosixPath(relative_meta.parent.as_posix()) / relative_path(name)))
                path = target_path(context["root"], relative)
                if path in seen:
                    continue
                seen.add(path)
                context["expected"].add(path)
                texture_jobs.append((scene_id, urljoin(context["base"], quote(str(relative), safe="/")), path))
        except Exception as error:
            context["errors"].append(f"{meta_path}: {error}")
            print(f"FAILED {scene_id} {meta_path}: {error}", flush=True)
    collision_jobs = [job for scene_id in chosen if scene_id in contexts for job in contexts[scene_id]["collision"]]
    _, texture_errors = run_jobs("Textures and collision", texture_jobs + collision_jobs, args.workers)
    for scene_id, error in texture_errors:
        contexts[scene_id]["errors"].append(error)
    print("Verifying scene files and calculating SHA-256 checksums...", flush=True)
    for scene_id in chosen:
        if scene_id not in contexts:
            continue
        context = contexts[scene_id]
        entries = []
        for path in sorted(context["expected"]):
            if not cached_valid(path):
                context["errors"].append(f"missing or invalid: {path.relative_to(context['root'])}")
                continue
            entries.append({"path": path.relative_to(context["root"]).as_posix(),
                            "bytes": path.stat().st_size, "sha256": file_digest(path)})
        collision_meta = context["root"] / "scene.voxel.json"
        collision_bin = context["root"] / "scene.voxel.bin"
        if collision_meta.exists() and collision_bin.exists():
            meta = json.loads(collision_meta.read_bytes())
            expected_bytes = (meta["nodeCount"] + meta["leafDataCount"]) * 4
            if collision_bin.stat().st_size != expected_bytes:
                context["errors"].append(f"collision binary has {collision_bin.stat().st_size} bytes; expected {expected_bytes}")
        try:
            lod_validation = inspect_lod_tree(context["root"])
        except (OSError, ValueError, KeyError, IndexError, TypeError) as error:
            context["errors"].append(f"LOD tree validation: {error}")
            lod_validation = None
        inventory = {"scene_id": scene_id, "lod_validation": lod_validation, "complete": not context["errors"],
                     "file_count": len(entries), "bytes": sum(entry["bytes"] for entry in entries),
                     "files": entries, "errors": context["errors"]}
        write_atomic(context["root"] / "download-inventory.json", json.dumps(inventory, indent=2).encode() + b"\n")
        print(f"{scene_id}: {'COMPLETE' if inventory['complete'] else 'INCOMPLETE'} | "
              f"{inventory['file_count']} files | {inventory['bytes'] / 1024**2:.1f} MiB | "
              f"{len(inventory['errors'])} errors", flush=True)
    return int(len(contexts) != len(chosen) or any(c["errors"] for c in contexts.values()))


if __name__ == "__main__":
    sys.exit(main())
