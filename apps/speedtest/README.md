# 1xBTS Speed Test

Small browser speed test for low data rate devices. It is designed for links
around or below 100 kbps and for old browsers.

## Run

```sh
go run .
```

Open `http://localhost:5656/`.

Set a different port with:

```sh
PORT=9090 go run .
```

## Docker Compose

In the 1xBTS compose stack this app runs as the `speedtest` service on port
`5656` internally. The `fou-nat` gateway exposes it to the host at
`http://localhost:${SPEEDTEST_PORT:-5656}/` and to mobile packet-data clients at
`http://speed/` or `http://speed.local.1xbts.org/` through the gateway nginx
proxy.

Mobile DNS for `speed` and `speed.local.1xbts.org` is served by Unbound in the
FOU gateway container. The PDSN advertises DNS servers from `config/pdsn.json`
(`packet.primary_dns` and `packet.secondary_dns`), which default to the packet
gateway resolver.

## Modes

- **Full** (default) times the transfer in the browser. Sizes up to 8 MiB.
- **Legacy HTML** is timed by the server and needs no scripting, for browsers
  such as Internet Explorer Mobile on Windows Mobile 5. Downloads run up to
  8 MiB and uploads up to 1 MiB, defaulting to 256 KiB down and 128 KiB up.

The mode comes from `?ui=full|legacy`, else a stored choice, else the user
agent. Every page has a switcher, and `?ui=auto` clears the stored choice.

Upload and download are separate tests and should be run one at a time.

## Legacy timing

The server records a start time in the page's continuation URL, hides the
payload in an HTML comment, and puts the continuation last so the page can only
advance after the final byte. The elapsed time is the difference between that
start and the callback request. Upload is timed directly from the request body.

Both figures include one round trip, so they read low, and more so as the link
gets faster. This mode is for a rough number on a slow link, not a precise one.

## Notes

The result is application-layer throughput between the device browser and this
server. Proxies, compression, TCP buffering, and radio scheduling can all affect
the numbers, especially on very slow links.

The `<noscript>` fallback inside the full page includes the callback request
latency, and uses a larger default download size to dilute it.

Sizes run from 4 KiB up. The full page defaults to 64 KiB, or 128 KiB in its
no-JavaScript fallback, chosen for roughly sub-150 kbps links. Legacy uploads
stop at 1 MiB because the payload rides in hidden form inputs, which the
browsers that mode targets cannot carry in bulk.
