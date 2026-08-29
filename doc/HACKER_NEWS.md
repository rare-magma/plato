# Hacker News

Plato includes a read-only Hacker News reader. Launch it from **Menu → Applications → Hacker News**.

The list uses Algolia's public Hacker News Search API and has three rolling windows:

- **Day** — stories submitted in the last 24 hours.
- **Week** — stories submitted in the last 7 days.
- **Month** — stories submitted in the last 30 days.

The tabs use Algolia's ranked `/search` results, and do not require configuration or authentication. Wi-Fi is required; Plato will request Wi-Fi when the app is opened while offline and Wi-Fi is disabled.

Tap a story to open its read-only thread in Plato's HTML reader. The thread includes the story facts, self-post text when available, the Algolia comment tree, and deleted or empty parents where they are needed to preserve replies. Link stories include **Open original article**. Tapping that link follows Plato's normal external-URL behavior, including `external-urls-queue` when configured.

There are no Hacker News login, submission, reply, edit, delete, voting, or mutation controls.
