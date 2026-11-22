#ifndef ZAWS_TESTING_H
#define ZAWS_TESTING_H
#include <aws/common/byte_buf.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* The TLS end of a loopback HTTPS server for tests. build.rs compiles it, with ZAWS_TESTING
 * defined, only for crt-sys's `testing` feature, so no product build contains it, and never for
 * Windows, which has no s2n.
 *
 * It serves with s2n directly, not through aws-c-io: on macOS aws-c-io's server TLS is Secure
 * Transport, which imports a server's identity into the user's keychain. Its certificates are made
 * when it is: a CA, and a leaf that CA issued to sts.example.invalid, localhost and 127.0.0.1,
 * valid from an hour before for a day, so none is ever stored or expires. */
struct zaws_test_tls;
/* NULL when the certificates or the configuration could not be made. */
struct zaws_test_tls *zaws_test_tls_new(void);
/* The CA's certificate, PEM, borrowed from `tls`. */
struct aws_byte_cursor zaws_test_tls_ca(const struct zaws_test_tls *tls);
/* Serves one connection on the connected socket `fd`, blocking: the handshake, one request (its
 * head and as many body bytes as its one Content-Length names) read into `out`, `response` written,
 * then reads until the client closes. The caller bounds each read with the socket's timeout and
 * closes `fd`. True when the request was read and answered whole, and then `*length` is its
 * length; false for a handshake that failed, a request longer than `capacity`, or a read or a
 * write that did. */
bool zaws_test_tls_serve(struct zaws_test_tls *tls, int fd, struct aws_byte_cursor response, uint8_t *out, size_t capacity, size_t *length);
void zaws_test_tls_destroy(struct zaws_test_tls *tls);
#endif
