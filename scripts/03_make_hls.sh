#!/usr/bin/env bash
# Lesson 03: chop the movie into 4-second HLS segments + a playlist (.m3u8).
set -euo pipefail
mkdir -p media/hls
ffmpeg -hide_banner -loglevel error -y -i media/sample.mp4 \
  -c:v libx264 -b:v 3000k -preset veryfast -g 120 -keyint_min 120 -sc_threshold 0 -c:a aac \
  -f hls -hls_time 4 -hls_playlist_type vod \
  -hls_segment_filename "media/hls/seg_%03d.ts" media/hls/index.m3u8
ls media/hls | head -5; echo "... $(ls media/hls | wc -l) files"
