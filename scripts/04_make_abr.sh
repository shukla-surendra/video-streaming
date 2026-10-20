#!/usr/bin/env bash
# Lesson 04: same movie in 3 qualities (the "ABR ladder") + one master playlist.
set -euo pipefail
out=media/abr; mkdir -p $out
ffmpeg -hide_banner -loglevel error -y -i media/sample.mp4 \
  -filter_complex "[0:v]split=3[a][b][c];[a]scale=-2:1080[v1080];[b]scale=-2:720[v720];[c]scale=-2:360[v360]" \
  -map "[v1080]" -c:v:0 libx264 -b:v:0 5000k -maxrate:v:0 5350k -bufsize:v:0 7500k \
  -map "[v720]"  -c:v:1 libx264 -b:v:1 2800k -maxrate:v:1 2996k -bufsize:v:1 4200k \
  -map "[v360]"  -c:v:2 libx264 -b:v:2 800k  -maxrate:v:2 856k  -bufsize:v:2 1200k \
  -map a:0 -map a:0 -map a:0 -c:a aac -b:a 128k \
  -preset veryfast -g 120 -keyint_min 120 -sc_threshold 0 \
  -f hls -hls_time 4 -hls_playlist_type vod \
  -var_stream_map "v:0,a:0,name:1080p v:1,a:1,name:720p v:2,a:2,name:360p" \
  -master_pl_name master.m3u8 \
  -hls_segment_filename "$out/%v/seg_%03d.ts" "$out/%v/index.m3u8"
cat $out/master.m3u8
du -sh $out/*/ 
