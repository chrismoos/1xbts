# edge-resolver

Tiny HTTP sidecar that nginx calls via `auth_request` to attach an authoritative
`X-1xBTS-MSISDN` header to inbound MMSC requests (and anything else that wants
the same attribution).

The flow lives in <https://1xbts.org/docs/guides/mms>. The short version:

```
phone → nginx (port 80)
          │  auth_request GET /_msisdn?ip=$remote_addr
          ▼
   edge-resolver  ──→  BSC.PdsnManagementService.GetPdsnSessionByIp
          │
          ▼  X-1xBTS-MSISDN: <subscriber phone number>
       nginx → upstream (Mbuni MMSC)
```

## Environment

| Var              | Default                   | Purpose                                   |
| ---------------- | ------------------------- | ----------------------------------------- |
| `BIND_ADDR`      | `127.0.0.1:8088`          | Local bind. Always loopback in production. |
| `MGMT_GRPC_ADDR` | `127.0.0.1:17016`         | BSC management gRPC endpoint.             |

## Failure modes

Always returns `200 OK`. On miss / parse error / gRPC error the
`X-1xBTS-MSISDN` header is sent empty, so `auth_request` doesn't block the
underlying request — downstream falls back to whatever default attribution it
uses.
