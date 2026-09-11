// sftp-server: the stdio SFTP server dropbear execs for the sftp subsystem —
// the L0 ops-channel file lane (#319, 2026-09-12). dropbear 2026.94 ships
// SFTP support compiled in (DROPBEAR_SFTP_SERVER=/usr/libexec/sftp-server):
// per connection it execs this binary with the SFTP protocol on stdin/stdout,
// exactly like OpenSSH's /usr/libexec/sftp-server. The protocol side is
// github.com/pkg/sftp's Server (SFTP v3 — the dialect every scp/sftp/GUI
// client speaks); this main is just the stdio glue. No flags: dropbear execs
// a fixed path with no args, so flags here would be dead config.
package main

import (
	"errors"
	"io"
	"log"
	"os"

	"github.com/pkg/sftp"
)

// stdioAdapter joins the two ends of the subsystem pipe into the single
// io.ReadWriteCloser pkg/sftp v1.13 wants (Close = client hangup, nothing
// for us to close — the fds belong to dropbear).
type stdioAdapter struct {
	io.Reader
	io.Writer
}

func (stdioAdapter) Close() error { return nil }

func main() {
	srv, err := sftp.NewServer(stdioAdapter{os.Stdin, os.Stdout})
	if err != nil {
		log.Fatal(err)
	}
	if err := srv.Serve(); err != nil && !errors.Is(err, io.EOF) {
		// stderr rides the SSH channel; a clean disconnect is io.EOF and
		// the common case — stay silent there, report the rest once.
		log.Print(err)
		os.Exit(1)
	}
}
