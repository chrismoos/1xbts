package main

import (
	"fmt"
	"html"
	"net/http"
	"net/http/httptest"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"
)

func assertPocketIESafe(t *testing.T, body string) {
	t.Helper()
	for _, banned := range []string{"<iframe", "<svg", "getElementById", "@media", "position:absolute", "new Image("} {
		if strings.Contains(body, banned) {
			t.Fatalf("legacy document uses %q, which Internet Explorer Mobile does not support", banned)
		}
	}
}

// Browsers follow the meta refresh.
func autoContinuation(t *testing.T, body, prefix string) string {
	t.Helper()
	re := regexp.MustCompile(`content="0;url=(` + regexp.QuoteMeta(prefix) + `[^"]*)"`)
	m := re.FindStringSubmatch(body)
	if m == nil {
		t.Fatalf("no automatic continuation to %s in %q", prefix, body)
	}
	return html.UnescapeString(m[1])
}

func TestLegacyFillCannotCloseTheHidingComment(t *testing.T) {
	if legacyFill == '-' || legacyFill == '<' || legacyFill == '>' {
		t.Fatalf("legacyFill %q can terminate the HTML comment that hides the payload", legacyFill)
	}
}

func TestLegacyDownloadDocHidesPayloadBeforeTheContinuation(t *testing.T) {
	next := continuation{auto: "/l/dlr?a=1", manual: "/l/dlr?a=1&via=manual"}
	doc := string(legacyDownloadDoc(modeLegacy, "Downloading", "intro", 4096, next))

	assertPocketIESafe(t, doc)

	if len(doc) != 4096 {
		t.Fatalf("document is %d bytes, want exactly 4096", len(doc))
	}

	open := strings.Index(doc, "<!--")
	close := strings.Index(doc, "-->")
	if open < 0 || close < 0 || close < open {
		t.Fatalf("payload is not wrapped in an HTML comment")
	}

	// Nothing that advances the page may precede the payload.
	for _, marker := range []string{"http-equiv=\"refresh\"", "<script", "Show result"} {
		if idx := strings.Index(doc, marker); idx < close {
			t.Fatalf("%q appears at %d, before the payload ends at %d", marker, idx, close)
		}
	}
}

func TestLegacyDownloadDocLayersTheContinuation(t *testing.T) {
	next := continuation{auto: "/l/dlr?a=1&b=2", manual: "/l/dlr?a=1&b=2&via=manual"}
	doc := string(legacyDownloadDoc(modeLegacy, "Downloading", "intro", 128, next))

	if !strings.Contains(doc, `<meta http-equiv="refresh" content="0;url=/l/dlr?a=1&amp;b=2">`) {
		t.Fatalf("missing meta refresh continuation: %q", doc)
	}
	if !strings.Contains(doc, `var u="/l/dlr?a=1&b=2";`) {
		t.Fatalf("missing script continuation: %q", doc)
	}
	if !strings.Contains(doc, `if(window.navigate){window.navigate(u);}`) {
		t.Fatalf("missing navigate fallback: %q", doc)
	}
	if !strings.Contains(doc, `href="/l/dlr?a=1&amp;b=2&amp;via=manual">Show result`) {
		t.Fatalf("missing manual continuation: %q", doc)
	}
}

func TestLegacyDownloadDocAlwaysOffersTheManualContinuation(t *testing.T) {
	next := continuation{auto: "/l/dlr?a=1", manual: "/l/dlr?a=1&via=manual"}
	doc := string(legacyDownloadDoc(modeLegacy, "Downloading", "intro", 128, next))

	if !strings.Contains(doc, "Show result") {
		t.Fatalf("missing the manual continuation: %q", doc)
	}
}

func TestLegacyDownloadEntryServesExactlyTheRequestedSize(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/l/dl?ui=legacy&bytes=262144", nil)
	rec := httptest.NewRecorder()

	s.legacyDownloadEntry(rec, req)

	res := rec.Result()
	body := rec.Body.String()
	if len(body) != 262144 {
		t.Fatalf("document is %d bytes, want exactly 262144", len(body))
	}
	if n, _ := strconv.Atoi(res.Header.Get("Content-Length")); n != len(body) {
		t.Fatalf("Content-Length = %s, body = %d bytes", res.Header.Get("Content-Length"), len(body))
	}
	if !strings.Contains(body, "/l/dlr?") {
		t.Fatalf("no result continuation: %q", body[:200])
	}
}

func TestLegacyDownloadRunReportsElapsedAndRate(t *testing.T) {
	s := testServer()

	entry := httptest.NewRecorder()
	s.legacyDownloadEntry(entry, httptest.NewRequest(http.MethodGet, "/l/dl?ui=legacy&bytes=262144", nil))
	resultURL := autoContinuation(t, entry.Body.String(), "/l/dlr")

	// Stands in for time on the air.
	time.Sleep(100 * time.Millisecond)

	rec := httptest.NewRecorder()
	s.legacyDownloadResult(rec, httptest.NewRequest(http.MethodGet, resultURL, nil))

	body := rec.Body.String()
	if rec.Result().StatusCode != http.StatusOK {
		t.Fatalf("status = %d: %q", rec.Result().StatusCode, body)
	}
	if !strings.Contains(body, "<td>Download</td><td>262144</td>") {
		t.Fatalf("expected the result table: %q", body)
	}

	// 256 KiB in ~0.1 s is roughly 21 Mbps, so the rate has to be far above a
	// value that would come from timing something other than the transfer.
	re := regexp.MustCompile(`<td>262144</td><td>([0-9.]+)</td><td>([0-9.]+)</td>`)
	m := re.FindStringSubmatch(body)
	if m == nil {
		t.Fatalf("no timing cells: %q", body)
	}
	seconds, err := strconv.ParseFloat(m[1], 64)
	if err != nil {
		t.Fatal(err)
	}
	if seconds < 0.09 || seconds > 5 {
		t.Fatalf("elapsed = %v s, want about 0.1", seconds)
	}
	kbps, err := strconv.ParseFloat(m[2], 64)
	if err != nil {
		t.Fatal(err)
	}
	if want := 262144 * 8 / seconds / 1000; kbps < want*0.99 || kbps > want*1.01 {
		t.Fatalf("kbps = %v, want about %v for %v s", kbps, want, seconds)
	}
	assertPocketIESafe(t, body)
}

func TestLegacyDownloadResultReplaysTheFirstCallback(t *testing.T) {
	s := testServer()
	startNS := time.Now().Add(-2 * time.Second).UnixNano()
	target := s.resultURL(modeLegacy, 262144, startNS).auto

	first := httptest.NewRecorder()
	s.legacyDownloadResult(first, httptest.NewRequest(http.MethodGet, target, nil))

	time.Sleep(30 * time.Millisecond)

	second := httptest.NewRecorder()
	s.legacyDownloadResult(second, httptest.NewRequest(http.MethodGet, target, nil))

	if first.Body.String() != second.Body.String() {
		t.Fatalf("a repeated callback re-timed the run:\nfirst:  %q\nsecond: %q", first.Body.String(), second.Body.String())
	}
}

func TestLegacyDownloadResultSendsStaleRunsBackToStart(t *testing.T) {
	s := testServer()
	startNS := time.Now().Add(-2 * time.Second).UnixNano()
	target := s.resultURL(modeLegacy, 262144, startNS).auto

	parsed, err := url.Parse(target)
	if err != nil {
		t.Fatal(err)
	}

	for _, field := range []string{"bytes", "t0", "via"} {
		t.Run(field, func(t *testing.T) {
			q := parsed.Query()
			switch field {
			case "bytes":
				q.Set("bytes", "4096")
			case "t0":
				q.Set("t0", strconv.FormatInt(startNS-int64(time.Second), 10))
			case "via":
				q.Set("via", viaManual)
			}
			tampered := parsed.Path + "?" + q.Encode()

			rec := httptest.NewRecorder()
			s.legacyDownloadResult(rec, httptest.NewRequest(http.MethodGet, tampered, nil))

			res := rec.Result()
			if res.StatusCode != http.StatusFound {
				t.Fatalf("status = %d, want 302 after changing %s", res.StatusCode, field)
			}
			if loc := res.Header.Get("Location"); loc != "/?ui=legacy" {
				t.Fatalf("Location = %q, want the legacy start page", loc)
			}
		})
	}
}

func TestLegacyDownloadResultLabelsManualRuns(t *testing.T) {
	s := testServer()
	startNS := time.Now().Add(-2 * time.Second).UnixNano()
	target := s.resultURL(modeLegacy, 262144, startNS).manual

	rec := httptest.NewRecorder()
	s.legacyDownloadResult(rec, httptest.NewRequest(http.MethodGet, target, nil))

	body := rec.Body.String()
	if rec.Result().StatusCode != http.StatusOK {
		t.Fatalf("status = %d: %q", rec.Result().StatusCode, body)
	}
	if strings.Contains(body, "Corrected:") {
		t.Fatalf("a hand-followed run must not be presented as a corrected measurement: %q", body)
	}
	if !strings.Contains(body, "lower bound") {
		t.Fatalf("a hand-followed run should say so: %q", body)
	}
}

func TestLegacyUploadFormUsesHiddenInputChunks(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/l/ul?ui=legacy&bytes=16384", nil)
	rec := httptest.NewRecorder()

	s.legacyUploadForm(rec, req)

	body := rec.Body.String()
	assertPocketIESafe(t, body)
	if strings.Contains(body, "<textarea") {
		t.Fatalf("Internet Explorer Mobile ignores sizing on controls, so the payload must not sit in a textarea")
	}

	re := regexp.MustCompile(`<input type="hidden" name="p(\d+)" value="([^"]*)">`)
	matches := re.FindAllStringSubmatch(body, -1)
	if len(matches) != 16384/legacyUploadChunk {
		t.Fatalf("got %d payload chunks, want %d", len(matches), 16384/legacyUploadChunk)
	}

	var total int
	for _, m := range matches {
		total += len(m[2])
	}
	if total != 16384 {
		t.Fatalf("chunks carry %d bytes, want 16384", total)
	}
}

func TestLegacyUploadResultTimesTheRequestBody(t *testing.T) {
	s := testServer()
	body := "p0=" + strings.Repeat("a", 4096)
	req := httptest.NewRequest(http.MethodPost, "/l/ulr?ui=legacy&bytes=4096", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()

	s.legacyUploadResult(rec, req)

	text := rec.Body.String()
	if rec.Result().StatusCode != http.StatusOK {
		t.Fatalf("status = %d: %q", rec.Result().StatusCode, text)
	}
	if !strings.Contains(text, "<td>Upload</td><td>4096</td>") {
		t.Fatalf("expected the upload result table, got %q", text)
	}
	assertPocketIESafe(t, text)
}

func TestLegacyDownloadAcceptsTheLargeSizesButUploadDoesNot(t *testing.T) {
	for _, n := range []int{2 * 1024 * 1024, 4 * 1024 * 1024, 8 * 1024 * 1024} {
		req := httptest.NewRequest(http.MethodGet, fmt.Sprintf("/l/dl?bytes=%d", n), nil)
		if _, err := legacySize(req, "bytes", legacyDefaultDownload, legacyMaxDownload); err != nil {
			t.Fatalf("download should accept %d bytes: %v", n, err)
		}
		if _, err := legacySize(req, "bytes", legacyDefaultUpload, legacyMaxUpload); err == nil {
			t.Fatalf("upload should reject %d bytes: the form would carry %d hidden inputs", n, n/legacyUploadChunk)
		}
	}

	req := httptest.NewRequest(http.MethodGet, "/l/dl?bytes=1048576", nil)
	if _, err := legacySize(req, "bytes", legacyDefaultUpload, legacyMaxUpload); err != nil {
		t.Fatalf("upload should still accept 1 MiB: %v", err)
	}
}

func TestLegacyIndexOffersLargeDownloadsOnly(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/?ui=legacy", nil)
	rec := httptest.NewRecorder()

	s.legacyIndex(rec, req, modeLegacy)

	body := rec.Body.String()
	forms := strings.SplitN(body, `action="/l/ul"`, 2)
	if len(forms) != 2 {
		t.Fatalf("expected both forms on the legacy index: %q", body)
	}
	download, upload := forms[0], forms[1]

	for _, want := range []string{"2097152", "4194304", "8388608"} {
		if !strings.Contains(download, want) {
			t.Fatalf("download select is missing %s: %q", want, download)
		}
		if strings.Contains(upload, want) {
			t.Fatalf("upload select should not offer %s: %q", want, upload)
		}
	}
}

// A large run has to stay valid for longer than it takes to transfer.
func TestLegacyResultWindowOutlastsTheLargestRun(t *testing.T) {
	const slowLinkBitsPerSecond = 100_000
	worst := time.Duration(float64(legacyMaxDownload*8) / slowLinkBitsPerSecond * float64(time.Second))
	if legacyResultTTL <= worst {
		t.Fatalf("result TTL %v is shorter than an %s run at %d kbps (%v)",
			legacyResultTTL, formatBytes(legacyMaxDownload), slowLinkBitsPerSecond/1000, worst)
	}
}

// The signing key is regenerated on restart, so a result page reloaded across
// one has a signature the server cannot verify.
func TestLegacyDownloadResultSurvivesAServerRestart(t *testing.T) {
	before := testServer()
	startNS := time.Now().Add(-time.Second).UnixNano()
	target := before.resultURL(modeLegacy, 262144, startNS).auto

	after := testServer()
	after.signingKey = []byte("a different key")

	rec := httptest.NewRecorder()
	after.legacyDownloadResult(rec, httptest.NewRequest(http.MethodGet, target, nil))

	res := rec.Result()
	if res.StatusCode != http.StatusFound {
		t.Fatalf("status = %d, want 302", res.StatusCode)
	}
	if loc := res.Header.Get("Location"); loc != "/?ui=legacy" {
		t.Fatalf("Location = %q, want the legacy start page", loc)
	}
}
