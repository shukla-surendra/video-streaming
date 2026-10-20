#!/usr/bin/env bash
# Make a 60-second 1080p test "movie" (colour pattern with a seconds counter + film grain + beep).
# The grain makes it hard to compress, like real footage, so bitrates behave.
# No downloads, no copyright — ffmpeg draws it for us.
set -euo pipefail
mkdir -p media
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc=size=1920x1080:rate=30:duration=60" \
  -f lavfi -i "sine=frequency=440:duration=60" \
  -vf "noise=alls=25:allf=t" \
  -c:v libx264 -b:v 8M -maxrate 8M -bufsize 16M -preset veryfast -pix_fmt yuv420p -c:a aac -movflags +faststart \
  media/sample.mp4
ls -lh media/sample.mp4
