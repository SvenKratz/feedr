#!/usr/bin/env python3
"""Generate bounded fixtures and verify native media rendering/playback APIs."""
import json
import pathlib
import subprocess
import sys
import tempfile


def pdf_fixture(path):
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [4 0 R 6 0 R 8 0 R] /Count 3 >>", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for page, color in enumerate([(0.09, 0.19, 0.20), (0.20, 0.15, 0.23), (0.22, 0.20, 0.12)], 1):
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {len(objects)+2} 0 R >>".encode())
        content = (f"{color[0]} {color[1]} {color[2]} rg 0 0 612 792 re f\n"
                   "0.85 0.91 0.94 rg BT /F1 30 Tf 54 698 Td (Feedr / PDF preview) Tj ET\n"
                   f"BT /F1 18 Tf 54 642 Td (Page {page} of 3) Tj ET\n"
                   "BT /F1 13 Tf 54 580 Td (Navigate pages, zoom, and inspect thumbnails.) Tj ET\n"
                   "BT /F1 13 Tf 54 552 Td (This document stays inside the Feedr sidebar.) Tj ET\n").encode()
        objects.append(f"<< /Length {len(content)} >>\nstream\n".encode()+content+b"endstream")
    data = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for index, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{index} 0 obj\n".encode()+obj+b"\nendobj\n")
    start = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        data.extend(f"{offset:010d} 00000 n \n".encode())
    data.extend(f"trailer << /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n".encode())
    path.write_bytes(data)


def run(binary, root):
    pdf = root / "media-smoke.pdf"
    pdf_fixture(pdf)
    result = json.loads(subprocess.check_output([binary, "media-check", str(pdf)], text=True))
    assert result["pages"] == 3 and result["first_page"][0] > 0 and result["last_page"][1] > 0, result
    print("PASS: three-page PDF renders first and last pages", flush=True)
    for extension, video, audio in [("mp4", "libx264", "aac"), ("mov", "libx264", "aac"), ("webm", "libvpx-vp9", "libopus"), ("mkv", "libx264", "libopus")]:
        path = root / f"media-smoke.{extension}"
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "4", "-c:v", video, "-pix_fmt", "yuv420p", "-c:a", audio, str(path)], check=True)
        result = json.loads(subprocess.check_output([binary, "media-check", str(path)], text=True, timeout=30))
        assert result["audio_track"] and result["seek_and_pause"] and result["duration"] >= 3.9, result
        print(f"PASS: {extension} ({video}/{audio}) embedded frames, audio track, seek and pause", flush=True)
    bad = root / "corrupt.pdf"
    bad.write_text("This is not a PDF")
    assert subprocess.run([binary, "media-check", str(bad)], capture_output=True).returncode != 0
    print("PASS: corrupt PDF reports a recoverable error", flush=True)


if __name__ == "__main__":
    binary = str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else "target/debug/feedr").resolve())
    if len(sys.argv)>2:
        root = pathlib.Path(sys.argv[2]).resolve()
        root.mkdir(parents=True, exist_ok=True)
        run(binary, root)
    else:
        with tempfile.TemporaryDirectory(prefix="feedr-media-") as directory:
            run(binary, pathlib.Path(directory))
