# Miniflux

Plato includes a reader for unread entries from a [Miniflux](https://miniflux.app/) account.
It can show unread entries from every category or filter them by category.

## Configuration

Create an API key in Miniflux under *Settings → API Keys*, then add the following to
Plato's `Settings.toml`:

```toml
[miniflux]
domain = "https://miniflux.example.org"
api-key = "your-api-key"
```

The domain must be the address of the Miniflux installation, without `/v1`.

## Usage

Open *Menu → Applications → Miniflux*. Tap the title to choose a category or refresh
the unread list. Opening an entry marks it as read in Miniflux. While reading a
Miniflux entry, tap the title and select *Mark as Unread* to restore its unread status.

The application requires a network connection. Plato will enable WiFi when needed.
