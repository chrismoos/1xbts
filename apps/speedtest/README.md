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

- JavaScript mode uses old-style JavaScript only: `Date`, `Image`, forms, and a
  hidden iframe. The 4 MiB and 8 MiB choices are long enough to measure
  steady-state HRPD throughput after TCP slow start.
- No-JavaScript mode is exposed through `<noscript>` and uses plain links and
  forms. Its download test streams hidden payload bytes before an iframe result
  callback, so timing waits for the browser to parse past the payload instead of
  relying on server flush timing.

Upload and download are separate tests and should be run one at a time.

## Notes

The result is application-layer throughput between the device browser and this
server. Proxies, compression, TCP buffering, and radio scheduling can all affect
the numbers, especially on very slow links.

No-JavaScript download results include the callback request latency. The default
no-JavaScript download size is larger than the JavaScript default to make that
latency a smaller part of the measurement.

Available test sizes range from 4 KiB through 2 MiB. The defaults are chosen
for roughly sub-150 kbps links: 64 KiB for JavaScript mode and 128 KiB for the
no-JavaScript download fallback.
