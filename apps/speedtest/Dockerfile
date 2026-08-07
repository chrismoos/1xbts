FROM golang:1.22-bookworm AS build

ARG TARGETOS
ARG TARGETARCH

WORKDIR /src
COPY go.mod ./
COPY *.go ./
RUN go test ./...
RUN test -n "${TARGETOS}" && test -n "${TARGETARCH}" && CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} go build -trimpath -ldflags="-s -w" -o /out/speedtest .

FROM gcr.io/distroless/static-debian12:nonroot

ENV PORT=5656
EXPOSE 5656
COPY --from=build /out/speedtest /speedtest
USER nonroot:nonroot
ENTRYPOINT ["/speedtest"]
