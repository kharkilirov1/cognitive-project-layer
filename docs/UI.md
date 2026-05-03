# Local dashboard UI

CPL includes a zero-dependency local web dashboard served by the same HTTP
server as the API.

Start it:

```bash
cpl serve --root . --host 127.0.0.1 --port 3878
```

Open:

```text
http://127.0.0.1:3878/ui
```

Aliases:

- `/`
- `/ui`
- `/dashboard`

## What it shows

- health and project root;
- scan summary and languages;
- `cpl doctor` status;
- SQLite index freshness;
- vector DB backend/model/record count;
- hybrid retrieval output;
- SQLite FTS search results;
- embedding search results;
- `.cpl/eval-results/*.json` benchmark/eval history.

## Actions

The dashboard can call existing local maintenance endpoints:

- `POST /index/refresh`
- `POST /embeddings/refresh`

These are local project-cache operations. They update `.cpl/index.sqlite` and
`.cpl/vectors.sqlite`; they do not send code externally unless the configured
embedding backend itself is external.

## Safety

Keep the server bound to loopback unless you intentionally want another process
or machine to access project context:

```bash
cpl serve --root . --host 127.0.0.1
```

The dashboard has no external CDN, no analytics, and no bundled third-party
frontend dependencies.
