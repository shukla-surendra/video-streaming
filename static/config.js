// Where the viewer site and the admin panel live. Empty = same origin (cargo run).
// In docker compose, nginx serves its own version of this file from SITE_URL / ADMIN_URL.
window.VSTREAM = { siteUrl: '', adminUrl: '' };
