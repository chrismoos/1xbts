package main

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"
)

func testServer() *server {
	return &server{
		downloadPayload: makeGIFPayload(maxSize),
		uploadPayload:   makeTextPayload(maxSize),
		signingKey:      []byte("test-signing-key"),
	}
}

func TestGIFPayloadExactSize(t *testing.T) {
	for _, n := range sizes {
		payload := makeGIFPayload(n)
		if len(payload) != n {
			t.Fatalf("payload length = %d, want %d", len(payload), n)
		}
		if string(payload[:6]) != "GIF89a" {
			t.Fatalf("payload does not start with GIF89a")
		}
		if payload[len(payload)-1] != ';' {
			t.Fatalf("payload does not end with GIF trailer")
		}
	}
}

func TestTextPayloadExactSizeAndFormSafe(t *testing.T) {
	payload := makeTextPayload(maxSize)
	if len(payload) != maxSize {
		t.Fatalf("payload length = %d, want %d", len(payload), maxSize)
	}
	for _, b := range payload {
		if !(b >= '0' && b <= '9') && !(b >= 'A' && b <= 'Z') && !(b >= 'a' && b <= 'z') {
			t.Fatalf("payload contains non form-safe byte %q", b)
		}
	}
}

func TestSizeOptionsIncludeLargeHRPDRuns(t *testing.T) {
	opts := sizeOptions()
	var hasOneMiB, hasTwoMiB, hasFourMiB, hasEightMiB bool
	for _, opt := range opts {
		switch opt.Bytes {
		case 1024 * 1024:
			hasOneMiB = true
		case 2 * 1024 * 1024:
			hasTwoMiB = true
		case 4 * 1024 * 1024:
			hasFourMiB = true
		case 8 * 1024 * 1024:
			hasEightMiB = true
		}
	}
	if !hasOneMiB || !hasTwoMiB || !hasFourMiB || !hasEightMiB {
		t.Fatalf(
			"size options missing large HRPD runs: 1MiB=%v 2MiB=%v 4MiB=%v 8MiB=%v opts=%v",
			hasOneMiB,
			hasTwoMiB,
			hasFourMiB,
			hasEightMiB,
			opts,
		)
	}
}

func TestDownloadBinHeadersAndSize(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/download.bin?bytes=4096&nonce=x", nil)
	rec := httptest.NewRecorder()

	s.downloadBin(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	if res.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200", res.StatusCode)
	}
	if len(body) != 4096 {
		t.Fatalf("body length = %d, want 4096", len(body))
	}
	if res.Header.Get("Content-Type") != "image/gif" {
		t.Fatalf("content type = %q, want image/gif", res.Header.Get("Content-Type"))
	}
	if res.Header.Get("Cache-Control") == "" {
		t.Fatalf("missing cache-control")
	}
}

func TestWarmupGIFIsSmallUncachedImage(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/warmup.gif?nonce=x", nil)
	rec := httptest.NewRecorder()

	s.warmupGIF(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	if res.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200", res.StatusCode)
	}
	if len(body) != 43 {
		t.Fatalf("body length = %d, want 43", len(body))
	}
	if res.Header.Get("Content-Type") != "image/gif" {
		t.Fatalf("content type = %q, want image/gif", res.Header.Get("Content-Type"))
	}
	if res.Header.Get("Cache-Control") == "" {
		t.Fatalf("missing cache-control")
	}
}

func TestDownloadStartUsesHiddenPayloadAndIframeCallback(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/download-start?bytes=4096", nil)
	rec := httptest.NewRecorder()

	s.downloadStart(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	text := string(body)
	if strings.Contains(text, "<pre>") {
		t.Fatalf("download fallback should not render visible payload progress")
	}
	if !strings.Contains(text, "<!--") || !strings.Contains(text, "-->") {
		t.Fatalf("download fallback should stream payload inside an HTML comment")
	}
	if strings.Contains(text, "Download</td><td>server observed") {
		t.Fatalf("download start page should not calculate a server-flush result")
	}
	if !strings.Contains(text, "<iframe") || !strings.Contains(text, "/download-result?bytes=4096") {
		t.Fatalf("expected iframe callback URL, got %q", text)
	}
}

func TestDownloadResultVerifiesSignedTimestamp(t *testing.T) {
	s := testServer()
	startNS := time.Now().Add(-2 * time.Second).UnixNano()
	req := httptest.NewRequest(http.MethodGet, "/download-result?bytes=4096&start="+strconv.FormatInt(startNS, 10)+"&sig="+s.signDownload(4096, startNS), nil)
	rec := httptest.NewRecorder()

	s.downloadResult(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	text := string(body)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("status = %d, want 200: %q", res.StatusCode, text)
	}
	if !strings.Contains(text, "Download</td><td>browser callback") {
		t.Fatalf("expected browser callback result table, got %q", text)
	}
}

func TestDownloadResultRejectsBadSignature(t *testing.T) {
	s := testServer()
	startNS := time.Now().Add(-2 * time.Second).UnixNano()
	req := httptest.NewRequest(http.MethodGet, "/download-result?bytes=4096&start="+strconv.FormatInt(startNS, 10)+"&sig=bad", nil)
	rec := httptest.NewRecorder()

	s.downloadResult(rec, req)

	if rec.Result().StatusCode != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400", rec.Result().StatusCode)
	}
}

func TestUploadRejectsOversizedBody(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodPost, "/upload", strings.NewReader(strings.Repeat("x", maxSize+5000)))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()

	s.upload(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "Upload was too large") {
		t.Fatalf("expected oversize error, got %q", string(body))
	}
}

func TestJSUploadReturnsCallbackAfterDrainingBody(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodPost, "/upload?js=1&bytes=4096&token=abc123", strings.NewReader("payload="+strings.Repeat("x", 4096)))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()

	s.upload(rec, req)

	res := rec.Result()
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	text := string(body)
	if !strings.Contains(text, "parent.speedTestUploadDone('abc123'") {
		t.Fatalf("expected JS callback response, got %q", text)
	}
	if !strings.Contains(text, ",4096,") {
		t.Fatalf("expected payload byte count in callback, got %q", text)
	}
}

func TestFormatKbps(t *testing.T) {
	got := formatKbps(1000, time.Second)
	if got != "8.0" {
		t.Fatalf("formatKbps = %q, want 8.0", got)
	}
}
