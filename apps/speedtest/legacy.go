package main

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"html/template"
	"io"
	"log"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	legacyMaxDownload     = 8 * 1024 * 1024
	legacyDefaultDownload = 256 * 1024
	legacyDefaultUpload   = 128 * 1024
	legacyUploadChunk     = 4 * 1024

	// Uplink is the slow direction and every byte rides in a hidden input, so
	// the upload form stays small enough for the browsers this mode targets.
	legacyMaxUpload = 1024 * 1024

	// An 8 MiB run at 100 kbps takes about 11 minutes, so the window a result
	// URL stays valid for has to outlast the transfer it is timing.
	legacyResultTTL = 30 * time.Minute
)

// Must never close an HTML comment.
const legacyFill = 'x'

const (
	viaAuto   = "auto"
	viaManual = "manual"
)

// Signed separately: a hand-followed run must not read as a measurement.
type continuation struct {
	auto   string
	manual string
}

// Both continuations can fire, so later callbacks replay the first arrival.
type arrivalCache struct {
	mu   sync.Mutex
	seen map[string]time.Time
}

func newArrivalCache() *arrivalCache {
	return &arrivalCache{seen: make(map[string]time.Time)}
}

func (c *arrivalCache) firstArrival(sig string, now time.Time) time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	for key, at := range c.seen {
		if now.Sub(at) > legacyResultTTL {
			delete(c.seen, key)
		}
	}
	if at, ok := c.seen[sig]; ok {
		return at
	}
	c.seen[sig] = now
	return now
}

func (s *server) signLegacy(scope string, fields ...string) string {
	mac := hmac.New(sha256.New, s.signingKey)
	fmt.Fprintf(mac, "%s\x00%s", scope, strings.Join(fields, "\x00"))
	return hex.EncodeToString(mac.Sum(nil))
}

func (s *server) verifyLegacy(sig, scope string, fields ...string) bool {
	got, err := hex.DecodeString(sig)
	if err != nil {
		return false
	}
	want, err := hex.DecodeString(s.signLegacy(scope, fields...))
	if err != nil {
		return false
	}
	return hmac.Equal(got, want)
}

func legacySize(r *http.Request, name string, fallback, max int) (int, error) {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return fallback, nil
	}
	n, err := strconv.Atoi(raw)
	if err != nil || n <= 0 {
		return 0, fmt.Errorf("invalid size")
	}
	for _, allowed := range sizes {
		if n == allowed && n <= max {
			return n, nil
		}
	}
	return 0, fmt.Errorf("unsupported size")
}

func legacySizeOptions(max int) []sizeOption {
	out := make([]sizeOption, 0, len(sizes))
	for _, n := range sizes {
		if n <= max {
			out = append(out, sizeOption{Bytes: n, Label: formatBytes(int64(n))})
		}
	}
	return out
}

func (s *server) resultURL(mode uiMode, n int, startNS int64) continuation {
	build := func(via string) string {
		q := url.Values{}
		q.Set(modeCookie, string(mode))
		q.Set("bytes", strconv.Itoa(n))
		q.Set("t0", strconv.FormatInt(startNS, 10))
		q.Set("via", via)
		q.Set("n", nonce())
		q.Set("sig", s.signLegacy("result", strconv.Itoa(n), strconv.FormatInt(startNS, 10), via))
		return "/l/dlr?" + q.Encode()
	}
	return continuation{auto: build(viaAuto), manual: build(viaManual)}
}

func (s *server) writeLegacyDoc(w http.ResponseWriter, doc []byte) {
	setNoStore(w)
	w.Header().Set("Content-Type", "text/html; charset=us-ascii")
	w.Header().Set("Content-Language", "en")
	w.Header().Set("Content-Length", strconv.Itoa(len(doc)))
	_, _ = w.Write(doc)
}

// Padded to exactly totalBytes so the reported size is what crossed the link.
func legacyDownloadDoc(mode uiMode, heading, intro string, totalBytes int, next continuation) []byte {
	markup := legacyDownloadMarkup(mode, heading, intro, 0, next)
	fill := totalBytes - len(markup)
	if fill < 0 {
		fill = 0
	}
	return legacyDownloadMarkup(mode, heading, intro, fill, next)
}

// Continuation goes last: the browser cannot reach it before the final payload
// byte. Built whole for an exact Content-Length.
func legacyDownloadMarkup(mode uiMode, heading, intro string, payloadLen int, next continuation) []byte {
	var b bytes.Buffer
	b.Grow(payloadLen + 4096)
	b.WriteString(legacyHead(heading))
	b.WriteString(`<p class="tag">`)
	b.WriteString(template.HTMLEscapeString(heading))
	b.WriteString(`</p><p class="hint">`)
	b.WriteString(template.HTMLEscapeString(intro))
	b.WriteString("</p>\n<!--")
	b.Write(bytes.Repeat([]byte{legacyFill}, payloadLen))
	b.WriteString("-->\n")

	escaped := template.HTMLEscapeString(next.auto)
	b.WriteString(`<meta http-equiv="refresh" content="0;url=`)
	b.WriteString(escaped)
	b.WriteString("\">\n")
	b.WriteString(`<script type="text/javascript">` + "\n")
	b.WriteString(`var u="` + jsString(next.auto) + `";` + "\n")
	b.WriteString("location.href=u;\n")
	b.WriteString("if(window.navigate){window.navigate(u);}\n")
	b.WriteString("</script>\n")

	// Last resort for browsers that follow neither.
	b.WriteString(`<p class="nav"><a href="`)
	b.WriteString(template.HTMLEscapeString(next.manual))
	b.WriteString(`">Show result</a></p>`)
	b.WriteString(legacyFoot(mode))
	return b.Bytes()
}

func (s *server) legacyIndex(w http.ResponseWriter, r *http.Request, mode uiMode) {
	setHTMLHeaders(w)
	data := struct {
		Mode            uiMode
		DownloadSizes   []sizeOption
		UploadSizes     []sizeOption
		DefaultDownload int
		DefaultUpload   int
		Head            template.HTML
		Foot            template.HTML
	}{
		Mode:            mode,
		DownloadSizes:   legacySizeOptions(legacyMaxDownload),
		UploadSizes:     legacySizeOptions(legacyMaxUpload),
		DefaultDownload: legacyDefaultDownload,
		DefaultUpload:   legacyDefaultUpload,
		Head:            template.HTML(legacyHead("1xBTS Speed Test")),
		Foot:            template.HTML(legacyFoot(mode)),
	}
	if err := legacyIndexTemplate.Execute(w, data); err != nil {
		log.Printf("render legacy index: %v", err)
	}
}

// A stale result URL is usually just a reload or a server restart, which
// regenerates the signing key. Send those back to the start rather than showing
// an error a phone browser cannot act on. 302 rather than 303 because the
// browsers this mode targets predate HTTP/1.1.
func (s *server) legacyRestart(w http.ResponseWriter, r *http.Request, mode uiMode, reason string) {
	log.Printf("legacy %s: %s, returning to the start page", r.URL.Path, reason)
	http.Redirect(w, r, "/?ui="+string(mode), http.StatusFound)
}

func (s *server) legacyDownloadEntry(w http.ResponseWriter, r *http.Request) {
	mode := resolveMode(r)
	n, err := legacySize(r, "bytes", legacyDefaultDownload, legacyMaxDownload)
	if err != nil {
		s.legacyRestart(w, r, mode, err.Error())
		return
	}

	next := s.resultURL(mode, n, time.Now().UnixNano())
	doc := legacyDownloadDoc(
		mode,
		"Downloading",
		fmt.Sprintf("Receiving %s. The result appears when the transfer finishes.", formatBytes(int64(n))),
		n,
		next,
	)
	s.writeLegacyDoc(w, doc)
}

func (s *server) legacyDownloadResult(w http.ResponseWriter, r *http.Request) {
	mode := resolveMode(r)
	q := r.URL.Query()
	n, err := legacySize(r, "bytes", legacyDefaultDownload, legacyMaxDownload)
	if err != nil {
		s.legacyRestart(w, r, mode, err.Error())
		return
	}
	startNS, err := strconv.ParseInt(q.Get("t0"), 10, 64)
	if err != nil || startNS <= 0 {
		s.legacyRestart(w, r, mode, "invalid timestamp")
		return
	}
	via := q.Get("via")
	sig := q.Get("sig")
	if !s.verifyLegacy(sig, "result", strconv.Itoa(n), strconv.FormatInt(startNS, 10), via) {
		s.legacyRestart(w, r, mode, "invalid signature")
		return
	}

	start := time.Unix(0, startNS)
	arrival := s.arrivals.firstArrival(sig, time.Now())
	if !legacyTimestampFresh(start, arrival) {
		s.legacyRestart(w, r, mode, "expired test")
		return
	}

	measured := arrival.Sub(start)
	data := legacyResultData{
		Mode:      mode,
		Title:     "Download Result",
		Direction: "Download",
		Bytes:     int64(n),
		Seconds:   formatSeconds(measured),
		Kbps:      formatKbps(int64(n), measured),
	}

	if via == viaManual {
		data.Note = "This browser did not advance the page on its own, so the figure includes the time taken to tap Show result. Treat it as a lower bound."
	}

	setHTMLHeaders(w)
	if err := legacyResultTemplate.Execute(w, data); err != nil {
		log.Printf("render legacy result: %v", err)
	}
}

func (s *server) legacyUploadForm(w http.ResponseWriter, r *http.Request) {
	mode := resolveMode(r)
	n, err := legacySize(r, "bytes", legacyDefaultUpload, legacyMaxUpload)
	if err != nil {
		s.legacyRestart(w, r, mode, err.Error())
		return
	}

	// IE Mobile ignores sizing on form controls; a textarea would render full size.
	chunks := make([]string, 0, n/legacyUploadChunk+1)
	for offset := 0; offset < n; offset += legacyUploadChunk {
		end := offset + legacyUploadChunk
		if end > n {
			end = n
		}
		chunks = append(chunks, string(s.uploadPayload[offset:end]))
	}

	setHTMLHeaders(w)
	data := struct {
		Mode   uiMode
		Bytes  int
		Label  string
		Action string
		Chunks []string
		Head   template.HTML
		Foot   template.HTML
	}{
		Mode:   mode,
		Bytes:  n,
		Label:  formatBytes(int64(n)),
		Action: fmt.Sprintf("/l/ulr?ui=%s&bytes=%d&n=%s", mode, n, nonce()),
		Chunks: chunks,
		Head:   template.HTML(legacyHead("Upload Test")),
		Foot:   template.HTML(legacyFoot(mode)),
	}
	if err := legacyUploadTemplate.Execute(w, data); err != nil {
		log.Printf("render legacy upload form: %v", err)
	}
}

func (s *server) legacyUploadResult(w http.ResponseWriter, r *http.Request) {
	mode := resolveMode(r)
	if r.Method != http.MethodPost {
		s.legacyRestart(w, r, mode, "upload result reached without a POST")
		return
	}
	n, err := legacySize(r, "bytes", legacyDefaultUpload, legacyMaxUpload)
	if err != nil {
		s.legacyRestart(w, r, mode, err.Error())
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, int64(legacyMaxUpload*2+4096))
	start := time.Now()
	read, copyErr := io.Copy(io.Discard, r.Body)
	elapsed := time.Since(start)

	setHTMLHeaders(w)
	data := legacyResultData{
		Mode:      mode,
		Title:     "Upload Result",
		Direction: "Upload",
	}
	if copyErr != nil {
		data.Title = "Upload Error"
		data.Error = "Upload was too large or could not be read."
	} else {
		data.Bytes = int64(n)
		data.Seconds = formatSeconds(elapsed)
		data.Kbps = formatKbps(int64(n), elapsed)
		data.Note = fmt.Sprintf("Timed by the server across %d bytes of request body.", read)
	}
	if err := legacyResultTemplate.Execute(w, data); err != nil {
		log.Printf("render legacy upload result: %v", err)
	}
}

func legacyTimestampFresh(start, now time.Time) bool {
	return !start.After(now.Add(5*time.Second)) && now.Sub(start) <= legacyResultTTL
}

type legacyResultData struct {
	Mode      uiMode
	Title     string
	Direction string
	Bytes     int64
	Seconds   string
	Kbps      string
	Note      string
	Error     string
}

func legacyHead(title string) string {
	return `<!doctype html>
<html lang="en">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=us-ascii">
<meta http-equiv="Content-Language" content="en">
<title>` + template.HTMLEscapeString(title) + `</title>
<style type="text/css">` + legacyCSS + `</style>
</head>
<body>
<table class="box" cellspacing="0" cellpadding="2" width="100%">
<tr><td class="bar">1xBTS Speed Test</td></tr>
<tr><td class="body">`
}

func legacyFoot(mode uiMode) string {
	return `<hr>` + modeSwitcherHTML(mode) + `<p class="nav"><a href="/?ui=` + string(mode) + `">Return</a></p>
</td></tr></table>
</body>
</html>`
}

// CSS Mobile Profile 1.0 only: no media queries, no positioning, nothing on
// form controls.
const legacyCSS = `
body{background:#0c0e14;color:#e2e8f0;font:12px Verdana,Arial,sans-serif;margin:4px}
a{color:#818cf8}
.box{background:#111827}
.bar{background:#0c0e14;color:#34d399;font-weight:bold}
.body{background:#111827}
.tag{font-weight:bold;color:#f1f5f9;margin:2px 0}
.hint{font-size:11px;color:#94a3b8;margin:2px 0}
.nav{font-size:11px;margin:2px 0}
.res{background:#020617;font-size:11px}
.res th{color:#34d399;text-align:left}
.big{color:#34d399;font-weight:bold}
.err{color:#f87171}
.modes{font-size:11px}
`

var legacyIndexTemplate = template.Must(template.New("legacyIndex").Parse(`{{.Head}}
<p class="tag">Link Tester</p>
<p class="hint">Timed by the server. Run one direction at a time.</p>

<p class="tag">Download</p>
<form action="/l/dl" method="get">
<input type="hidden" name="ui" value="{{.Mode}}">
<select name="bytes">
{{range .DownloadSizes}}<option value="{{.Bytes}}"{{if eq .Bytes $.DefaultDownload}} selected{{end}}>{{.Label}}</option>{{end}}
</select>
<input type="submit" value="Start">
</form>

<p class="tag">Upload</p>
<form action="/l/ul" method="get">
<input type="hidden" name="ui" value="{{.Mode}}">
<select name="bytes">
{{range .UploadSizes}}<option value="{{.Bytes}}"{{if eq .Bytes $.DefaultUpload}} selected{{end}}>{{.Label}}</option>{{end}}
</select>
<input type="submit" value="Start">
</form>
{{.Foot}}`))

var legacyUploadTemplate = template.Must(template.New("legacyUpload").Parse(`{{.Head}}
<p class="tag">Upload {{.Label}}</p>
<p class="hint">Press Start and wait for the result page.</p>
<form action="{{.Action}}" method="post">
{{range $i, $chunk := .Chunks}}<input type="hidden" name="p{{$i}}" value="{{$chunk}}">
{{end}}<p><input type="submit" value="Start Upload"></p>
</form>
{{.Foot}}`))

var legacyFuncs = template.FuncMap{
	"legacyHead": func(title string) template.HTML { return template.HTML(legacyHead(title)) },
	"legacyFoot": func(mode uiMode) template.HTML { return template.HTML(legacyFoot(mode)) },
}

var legacyResultTemplate = template.Must(template.New("legacyResult").Funcs(legacyFuncs).Parse(`{{legacyHead .Title}}
<p class="tag">{{.Title}}</p>
{{if .Error}}<p class="err">{{.Error}}</p>{{else}}
<table class="res" cellspacing="0" cellpadding="2" width="100%">
<tr><th>Dir</th><th>Bytes</th><th>Sec</th><th>kbps</th></tr>
<tr><td>{{.Direction}}</td><td>{{.Bytes}}</td><td>{{.Seconds}}</td><td>{{.Kbps}}</td></tr>
</table>
{{if .Note}}<p class="hint">{{.Note}}</p>{{end}}
{{end}}
{{legacyFoot .Mode}}`))
