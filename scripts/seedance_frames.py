"""Turns pairs of first-person stills into footage frames with ByteDance Seedance 2.0 on fal.ai.

    generate  Submit a start (and optional end) still to fal's Seedance 2.0 image-to-video queue, wait, and save
              the MP4. The API key comes from the FAL_KEY environment variable and is never printed.
    frames    Cut a video into frames with ffmpeg, trim the still lead-in and tail (frames that barely change),
              pick N evenly spaced frames from the motion, key each one with the footage_still converter (exact
              chroma green, 1280x720 WebP) and write a contact sheet.

Standard library only; needs ffmpeg (FFMPEG or on PATH) and the built footage_still example for `frames`.

    python scripts/seedance_frames.py generate --start a.webp --end b.webp --prompt "..." --out clip.mp4
    python scripts/seedance_frames.py frames --video clip.mp4 --count 25 --out-dir frames/
    python scripts/seedance_frames.py frames --video clip.mp4 --count 19 --first 45 --last 72 --spacing motion ...
"""
import argparse
import base64
import json
import mimetypes
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODELS = {
    "standard": "bytedance/seedance-2.0/image-to-video",
    "fast": "bytedance/seedance-2.0/fast/image-to-video",
    "2.5": "bytedance/seedance-2.5/image-to-video",
}
FOOTAGE_STILL = ROOT / ".artifacts" / "target-native" / "release" / "examples" / "footage_still.exe"


def data_uri(path):
    kind = mimetypes.guess_type(str(path))[0] or ("image/webp" if str(path).endswith(".webp") else "image/png")
    return "data:%s;base64,%s" % (kind, base64.b64encode(Path(path).read_bytes()).decode("ascii"))


def request(url, key, body=None):
    headers = {"Authorization": "Key " + key, "Accept": "application/json"}
    data = None
    if body is not None:
        data = json.dumps(body).encode("utf8")
        headers["Content-Type"] = "application/json"
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers), timeout=120) as reply:
        return json.loads(reply.read().decode("utf8"))


def generate(args):
    key = os.environ.get("FAL_KEY", "").strip()
    if not key:
        sys.exit("Set FAL_KEY to a fal.ai API key.")
    if args.prompt_file:
        # Windows PowerShell 5.1 can cut a long quoted argument short on its way to a native program, so long
        # prompts come from a file.
        args.prompt = Path(args.prompt_file).read_text(encoding="utf8").strip()
    if not args.prompt:
        sys.exit("Give --prompt or --prompt-file.")
    print("prompt: %d characters" % len(args.prompt), flush=True)
    body = {
        "prompt": args.prompt,
        "image_url": data_uri(args.start),
        "duration": str(args.duration),
        "resolution": args.resolution,
        "aspect_ratio": "16:9",
        "generate_audio": False,
    }
    if args.end:
        body["end_image_url"] = data_uri(args.end)
    model = MODELS[args.model]
    if args.model == "2.5":
        # Seedance 2.5 takes its aspect ratio from the start image.
        body.pop("aspect_ratio", None)
    try:
        submitted = request("https://queue.fal.run/" + model, key, body)
    except urllib.error.HTTPError as error:
        sys.exit("fal rejected the request (%s): %s" % (error.code, error.read().decode("utf8", "replace")[:800]))
    status_url, response_url = submitted["status_url"], submitted["response_url"]
    print("submitted %s request %s" % (model, submitted.get("request_id")), flush=True)
    started = time.time()
    while True:
        status = request(status_url, key)
        state = status.get("status")
        if state == "COMPLETED":
            break
        if state not in ("IN_QUEUE", "IN_PROGRESS"):
            sys.exit("fal request ended with status %r: %s" % (state, json.dumps(status)[:500]))
        if time.time() - started > args.timeout:
            sys.exit("timed out waiting for fal after %d s" % args.timeout)
        time.sleep(5)
    try:
        result = request(response_url, key)
    except urllib.error.HTTPError as error:
        sys.exit("fal result failed (%s): %s" % (error.code, error.read().decode("utf8", "replace")[:800]))
    video = result.get("video") or {}
    if not video.get("url"):
        sys.exit("fal returned no video: %s" % json.dumps(result)[:800])
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    # Record the request first: a failed download can be retried from fal's stored result.
    meta = {"model": model, "request_id": submitted.get("request_id"), "video_url": video["url"],
            "seed": result.get("seed"), "prompt": args.prompt, "start": str(args.start),
            "end": str(args.end) if args.end else None, "duration": args.duration, "resolution": args.resolution,
            "seconds_waited": round(time.time() - started)}
    out.with_suffix(".json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf8")
    try:
        with urllib.request.urlopen(video["url"], timeout=300) as reply:
            out.write_bytes(reply.read())
    except urllib.error.URLError:
        # Python's certificate bundle can lack the media CDN's issuer; the system curl uses the OS store.
        subprocess.run(["curl", "-s", "-f", "-L", "-o", str(out), video["url"]], check=True)
    print("saved %s (%d KiB) seed %s after %d s" % (out, out.stat().st_size // 1024, meta["seed"], meta["seconds_waited"]))


def ffmpeg():
    found = os.environ.get("FFMPEG") or shutil.which("ffmpeg")
    if not found:
        sys.exit("ffmpeg not found: set FFMPEG or put it on PATH.")
    return found


def probe_motion(video, size=(64, 36)):
    """Mean absolute change between consecutive frames, from tiny grey thumbnails."""
    width, height = size
    raw = subprocess.run([ffmpeg(), "-v", "error", "-i", str(video), "-vf", "scale=%d:%d,format=gray" % size,
                          "-f", "rawvideo", "-"], check=True, capture_output=True).stdout
    frame = width * height
    count = len(raw) // frame
    frames = [raw[i * frame:(i + 1) * frame] for i in range(count)]
    changes = [sum(abs(a - b) for a, b in zip(frames[i], frames[i - 1])) / frame for i in range(1, count)]
    return count, changes


def pick_frames(changes, first, last, count, spacing):
    """0-based frame indices from first to last. `time` spaces them evenly in time; `motion` spaces them evenly
    along the accumulated picture change, so a quick move and a slow settle get frames in proportion to how
    much they move (the state machine supplies its own easing)."""
    if count <= 1 or last <= first:
        return [first] * max(1, count)
    span = last - first
    if spacing == "time":
        return [first + round(span * i / (count - 1)) for i in range(count)]
    # Subtract the clip's noise floor (encoder shimmer on a still picture) before accumulating.
    floor = sorted(changes)[len(changes) // 10] if changes else 0
    steps = [max(0.0, changes[i - 1] - floor) for i in range(first + 1, last + 1)]  # change into frame i
    total = sum(steps)
    if total <= 0:
        return [first + round(span * i / (count - 1)) for i in range(count)]
    picks, travelled, frame = [first], 0.0, first
    for n in range(1, count - 1):
        target = total * n / (count - 1)
        while frame < last and travelled + steps[frame - first] < target:
            travelled += steps[frame - first]
            frame += 1
        picks.append(max(frame, picks[-1]))
    picks.append(last)
    return picks


def frames(args):
    out = Path(args.out_dir)
    raw_dir = out / "raw"
    if out.exists():
        shutil.rmtree(out)
    raw_dir.mkdir(parents=True)
    subprocess.run([ffmpeg(), "-v", "error", "-i", args.video, "-fps_mode", "passthrough", str(raw_dir / "%04d.png")],
                   check=True)
    extracted = sorted(raw_dir.glob("*.png"))
    count, changes = probe_motion(args.video)
    # Trim the stretches before the motion starts and after it ends: frames that change less than a small
    # fraction of the busiest frame-to-frame change.
    peak = max(changes) if changes else 0
    moving = [i + 1 for i, c in enumerate(changes) if peak > 0 and c >= peak * args.still_fraction]
    first = max(0, (moving[0] - 1) if moving and args.trim else 0)
    last = min(len(extracted) - 1, moving[-1] if moving and args.trim else len(extracted) - 1)
    # A hand-picked range (1-based video frames) overrides the automatic trim.
    if args.first:
        first = max(0, args.first - 1)
    if args.last:
        last = min(len(extracted) - 1, args.last - 1)
    picks = pick_frames(changes, first, last, args.count, args.spacing)
    selected = out / "frames"
    selected.mkdir()
    for n, index in enumerate(picks, 1):
        target = selected / ("%04d.webp" % n)
        subprocess.run([str(FOOTAGE_STILL), str(extracted[index]), str(target)], check=True, capture_output=True)
    sheet = out / "contact.png"
    columns = min(8, len(picks))
    rows = (len(picks) + columns - 1) // columns
    subprocess.run([ffmpeg(), "-v", "error", "-y", "-framerate", "1", "-i", str(selected / "%04d.webp"),
                    "-vf", "scale=320:180,tile=%dx%d" % (columns, rows), "-frames:v", "1", str(sheet)], check=True)
    report = {"video_frames": count, "motion_first": first + 1, "motion_last": last + 1, "picked": [p + 1 for p in picks],
              "peak_change": round(peak, 2), "changes": [round(c, 2) for c in changes]}
    (out / "report.json").write_text(json.dumps(report, indent=1) + "\n", encoding="utf8")
    print("%d frames in video; motion %d-%d; wrote %d keyed frames and %s"
          % (count, first + 1, last + 1, len(picks), sheet))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    g = sub.add_parser("generate")
    g.add_argument("--start", required=True)
    g.add_argument("--end")
    prompt = g.add_mutually_exclusive_group(required=True)
    prompt.add_argument("--prompt")
    prompt.add_argument("--prompt-file", help="UTF-8 text file holding the prompt (use this for long prompts)")
    g.add_argument("--out", required=True)
    g.add_argument("--duration", type=int, default=4, choices=range(4, 31))
    g.add_argument("--resolution", default="720p", choices=["480p", "720p", "1080p"])
    g.add_argument("--model", default="standard", choices=sorted(MODELS))
    g.add_argument("--timeout", type=int, default=900)
    f = sub.add_parser("frames")
    f.add_argument("--video", required=True)
    f.add_argument("--count", type=int, required=True)
    f.add_argument("--out-dir", required=True)
    f.add_argument("--still-fraction", type=float, default=0.08)
    f.add_argument("--no-trim", dest="trim", action="store_false")
    f.add_argument("--first", type=int, help="first video frame to use (1-based), overriding the trim")
    f.add_argument("--last", type=int, help="last video frame to use (1-based), overriding the trim")
    f.add_argument("--spacing", default="time", choices=["time", "motion"])
    args = parser.parse_args()
    generate(args) if args.command == "generate" else frames(args)


if __name__ == "__main__":
    main()
