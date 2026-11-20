package mokuji

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
)

// Source is where a Mokuji file's bytes come from: a file on disk, an HTTP
// server, an object store client, a cache in front of one.
//
// ReadRange returns length bytes at offset, or fewer only at the end of the
// file. Every call is one range request, and the reader counts it as one.
type Source interface {
	ReadRange(ctx context.Context, offset, length int64) ([]byte, error)
}

// Sizer is implemented by a Source that knows the file's length, which lets
// the reader check offsets against it. Size returns a negative number while
// the length is not yet known, as for a URL before the first response.
type Sizer interface {
	Size() int64
}

// ReaderAtSource reads from an io.ReaderAt, such as a *bytes.Reader. When it
// also has a Size method, as *bytes.Reader and *io.SectionReader do, the
// source reports the length.
func ReaderAtSource(r io.ReaderAt) Source {
	return readerAtSource{r}
}

// BytesSource reads from a file held in memory.
func BytesSource(data []byte) Source {
	return readerAtSource{bytesReaderAt(data)}
}

type bytesReaderAt []byte

func (b bytesReaderAt) ReadAt(p []byte, off int64) (int, error) {
	if off >= int64(len(b)) {
		return 0, io.EOF
	}
	n := copy(p, b[off:])
	if n < len(p) {
		return n, io.EOF
	}
	return n, nil
}

func (b bytesReaderAt) Size() int64 { return int64(len(b)) }

type readerAtSource struct {
	r io.ReaderAt
}

func (s readerAtSource) ReadRange(ctx context.Context, offset, length int64) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	buf := make([]byte, length)
	n, err := s.r.ReadAt(buf, offset)
	if err != nil && !errors.Is(err, io.EOF) {
		return nil, err
	}
	return buf[:n], nil
}

func (s readerAtSource) Size() int64 {
	if sized, ok := s.r.(interface{ Size() int64 }); ok {
		return sized.Size()
	}
	return -1
}

// FileSource is a file on disk, read a range at a time.
//
// Positional reads, so a range costs that range and no more, and two
// goroutines reading the same file do not move a shared position under each
// other.
type FileSource struct {
	file *os.File
	size int64
}

// OpenFileSource opens the file at path.
func OpenFileSource(path string) (*FileSource, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	info, err := f.Stat()
	if err != nil {
		_ = f.Close()
		return nil, err
	}
	return &FileSource{file: f, size: info.Size()}, nil
}

func (s *FileSource) ReadRange(ctx context.Context, offset, length int64) ([]byte, error) {
	return readerAtSource{s.file}.ReadRange(ctx, offset, length)
}

// Size is the file's length.
func (s *FileSource) Size() int64 { return s.size }

// Close closes the file.
func (s *FileSource) Close() error { return s.file.Close() }

// RangeError is a server that answered a range request with something other
// than the range: usually the whole file, which is the one thing this format
// exists to avoid, so it is an error rather than a slow success.
type RangeError struct {
	URL    string
	Status int
}

func (e *RangeError) Error() string {
	return fmt.Sprintf("mokuji: the server ignored the Range header (status %d) for %s; Mokuji needs range requests to be honoured", e.Status, e.URL)
}

// HTTPSource is a file served over HTTP, read with Range requests.
type HTTPSource struct {
	URL string
	// Client makes the requests; nil means http.DefaultClient.
	Client *http.Client

	mu    sync.Mutex
	size  int64
	known bool
}

// NewHTTPSource reads the file at url with http.DefaultClient.
func NewHTTPSource(url string) *HTTPSource {
	return &HTTPSource{URL: url}
}

func (s *HTTPSource) ReadRange(ctx context.Context, offset, length int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.URL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Range", fmt.Sprintf("bytes=%d-%d", offset, offset+length-1))
	req.Header.Set("User-Agent", "mokuji-go")
	client := s.Client
	if client == nil {
		client = http.DefaultClient
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusPartialContent {
		return nil, &RangeError{URL: s.URL, Status: resp.StatusCode}
	}
	// `bytes first-last/total`: the total is the file's length.
	if _, total, ok := strings.Cut(resp.Header.Get("Content-Range"), "/"); ok {
		if n, err := strconv.ParseInt(total, 10, 64); err == nil {
			s.mu.Lock()
			if !s.known {
				s.size, s.known = n, true
			}
			s.mu.Unlock()
		}
	}
	// No more than was asked for, whatever the server sends.
	return io.ReadAll(io.LimitReader(resp.Body, length))
}

// Size is the file's length, once a response has said it; negative before.
func (s *HTTPSource) Size() int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.known {
		return -1
	}
	return s.size
}

func isURL(location string) bool {
	scheme, _, ok := strings.Cut(location, ":")
	if !ok {
		return false
	}
	scheme = strings.ToLower(scheme)
	return scheme == "http" || scheme == "https"
}
