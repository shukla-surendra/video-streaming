# Frontend and admin panel: static pages on nginx, which forwards /api and
# /stream to the backend. One file, two targets:
#   docker build -f docker/web.Dockerfile --target frontend .
#   docker build -f docker/web.Dockerfile --target admin .
# The official nginx image runs envsubst on /etc/nginx/templates/*.template
# at startup, so BACKEND_ADDR, SITE_URL etc. are set per container.

FROM nginx:1.30-alpine AS base
COPY static/app.css static/common.js /usr/share/nginx/html/static/
COPY static/vendor /usr/share/nginx/html/static/vendor

FROM base AS frontend
COPY static/index.html static/watch.html /usr/share/nginx/html/
COPY docker/nginx/frontend.conf.template /etc/nginx/templates/default.conf.template

FROM base AS admin
COPY static/admin.html /usr/share/nginx/html/
COPY docker/nginx/admin.conf.template /etc/nginx/templates/default.conf.template
