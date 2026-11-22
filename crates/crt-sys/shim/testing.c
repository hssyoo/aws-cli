#include "testing.h"
#include "crt_shim.h"
#include <openssl/evp.h>
#include <openssl/pem.h>
#include <openssl/x509.h>
#include <openssl/x509v3.h>
#include <pthread.h>
#include <s2n.h>

struct zaws_test_tls {
    struct aws_allocator *allocator;
    struct aws_byte_buf ca;
    struct s2n_config *config;
};

/* s2n once per process, unless the CRT already started it: aws-c-io's own test
 * (`s_tls_init_static_state`), as s2n refuses to disable its exit handler once it is started. On
 * Linux the CRT has started it, the caller holding the library; on macOS the CRT's TLS is Secure
 * Transport and nothing else starts it. */
static pthread_once_t s_s2n_once = PTHREAD_ONCE_INIT;
static void s_s2n_start(void) {
    if (s2n_disable_atexit() == S2N_SUCCESS) s2n_init();
}

static EVP_PKEY *s_key(void) {
    EVP_PKEY *key = NULL;
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_id(EVP_PKEY_EC, NULL);
    if (ctx && EVP_PKEY_keygen_init(ctx) > 0 && EVP_PKEY_CTX_set_ec_paramgen_curve_nid(ctx, NID_X9_62_prime256v1) > 0) EVP_PKEY_keygen(ctx, &key);
    EVP_PKEY_CTX_free(ctx);
    return key;
}
static bool s_extend(X509 *cert, X509 *issuer, int nid, const char *value) {
    X509V3_CTX ctx;
    X509V3_set_ctx_nodb(&ctx);
    X509V3_set_ctx(&ctx, issuer, cert, NULL, NULL, 0);
    X509_EXTENSION *extension = X509V3_EXT_conf_nid(NULL, &ctx, nid, value);
    bool added = extension && X509_add_ext(cert, extension, -1);
    X509_EXTENSION_free(extension);
    return added;
}
/* A certificate for `key` named `name`: the CA's, self-signed, when `issuer` is NULL, else a
 * server's that `issuer` signs with `issuer_key`. */
static X509 *s_certificate(EVP_PKEY *key, const char *name, X509 *issuer, EVP_PKEY *issuer_key) {
    X509 *cert = X509_new();
    if (!cert) return NULL;
    X509_NAME *subject = X509_get_subject_name(cert);
    bool made = X509_set_version(cert, 2) && ASN1_INTEGER_set(X509_get_serialNumber(cert), issuer ? 2 : 1) &&
        X509_gmtime_adj(X509_getm_notBefore(cert), -3600) && X509_gmtime_adj(X509_getm_notAfter(cert), 86400) &&
        X509_set_pubkey(cert, key) &&
        X509_NAME_add_entry_by_txt(subject, "CN", MBSTRING_ASC, (const unsigned char *)name, -1, -1, 0) &&
        X509_set_issuer_name(cert, issuer ? X509_get_subject_name(issuer) : subject);
    X509 *signer = issuer ? issuer : cert;
    if (issuer) {
        made = made && s_extend(cert, signer, NID_basic_constraints, "critical,CA:FALSE") &&
            s_extend(cert, signer, NID_key_usage, "critical,digitalSignature") &&
            s_extend(cert, signer, NID_ext_key_usage, "serverAuth") &&
            s_extend(cert, signer, NID_subject_alt_name, "DNS:sts.example.invalid,DNS:localhost,IP:127.0.0.1");
    } else {
        made = made && s_extend(cert, signer, NID_basic_constraints, "critical,CA:TRUE") &&
            s_extend(cert, signer, NID_key_usage, "critical,keyCertSign,cRLSign");
    }
    if (made && X509_sign(cert, issuer ? issuer_key : key, EVP_sha256()) > 0) return cert;
    X509_free(cert);
    return NULL;
}
/* `cert`'s PEM, or `key`'s when `cert` is NULL, NUL-terminated into `out`. */
static bool s_pem(struct aws_allocator *allocator, X509 *cert, EVP_PKEY *key, struct aws_byte_buf *out) {
    BIO *bio = BIO_new(BIO_s_mem());
    bool written = bio && (cert ? PEM_write_bio_X509(bio, cert) : PEM_write_bio_PrivateKey(bio, key, NULL, NULL, 0, NULL, NULL));
    char *data = NULL;
    long len = written ? BIO_get_mem_data(bio, &data) : 0;
    struct aws_byte_cursor pem = { .len = len > 0 ? (size_t)len : 0, .ptr = (uint8_t *)data };
    bool copied = len > 0 && !aws_byte_buf_init(out, allocator, pem.len + 1) && aws_byte_buf_write_from_whole_cursor(out, pem) &&
        aws_byte_buf_write_u8(out, 0);
    BIO_free(bio);
    return copied;
}
/* The CA's PEM into `tls`, and the leaf it issues with its key into the server's configuration. */
static bool s_identity(struct zaws_test_tls *tls) {
    EVP_PKEY *ca_key = s_key(), *leaf_key = s_key();
    X509 *ca = ca_key ? s_certificate(ca_key, "aws-cli loopback test CA", NULL, NULL) : NULL;
    X509 *leaf = ca && leaf_key ? s_certificate(leaf_key, "sts.example.invalid", ca, ca_key) : NULL;
    struct aws_byte_buf chain = { 0 }, key = { 0 };
    bool made = leaf && s_pem(tls->allocator, ca, NULL, &tls->ca) && s_pem(tls->allocator, leaf, NULL, &chain) &&
        s_pem(tls->allocator, NULL, leaf_key, &key) &&
        s2n_config_add_cert_chain_and_key(tls->config, (const char *)chain.buffer, (const char *)key.buffer) == S2N_SUCCESS;
    aws_byte_buf_clean_up(&chain);
    aws_byte_buf_clean_up_secure(&key);
    X509_free(leaf);
    X509_free(ca);
    EVP_PKEY_free(leaf_key);
    EVP_PKEY_free(ca_key);
    return made;
}
void zaws_test_tls_destroy(struct zaws_test_tls *tls) {
    if (!tls) return;
    if (tls->config) s2n_config_free(tls->config);
    aws_byte_buf_clean_up(&tls->ca);
    aws_mem_release(tls->allocator, tls);
}
struct zaws_test_tls *zaws_test_tls_new(void) {
    pthread_once(&s_s2n_once, s_s2n_start);
    struct aws_allocator *allocator = rust_crt_allocator();
    struct zaws_test_tls *tls = aws_mem_calloc(allocator, 1, sizeof(*tls));
    if (!tls) return NULL;
    tls->allocator = allocator;
    tls->config = s2n_config_new();
    if (tls->config && s_identity(tls)) return tls;
    zaws_test_tls_destroy(tls);
    return NULL;
}
struct aws_byte_cursor zaws_test_tls_ca(const struct zaws_test_tls *tls) {
    /* The PEM without its terminating NUL. */
    return (struct aws_byte_cursor){ .len = tls->ca.len ? tls->ca.len - 1 : 0, .ptr = tls->ca.buffer };
}
/* The length of the request at the start of `bytes` once it is whole, else 0: its head, then the
 * body its one Content-Length names, or none. */
static size_t s_request_length(struct aws_byte_cursor bytes) {
    struct aws_byte_cursor end = aws_byte_cursor_from_c_str("\r\n\r\n");
    struct aws_byte_cursor found;
    if (aws_byte_cursor_find_exact(&bytes, &end, &found)) return 0;
    size_t head = (size_t)(found.ptr - bytes.ptr) + end.len;
    struct aws_byte_cursor lines = { .len = head, .ptr = bytes.ptr };
    struct aws_byte_cursor line = { 0 };
    struct aws_byte_cursor name = aws_byte_cursor_from_c_str("content-length:");
    uint64_t body = 0;
    while (aws_byte_cursor_next_split(&lines, '\n', &line)) {
        if (line.len < name.len || !aws_array_eq_ignore_case(line.ptr, name.len, name.ptr, name.len)) continue;
        struct aws_byte_cursor value = { .len = line.len - name.len, .ptr = line.ptr + name.len };
        value = aws_byte_cursor_trim_pred(&value, aws_char_is_space);
        if (aws_byte_cursor_utf8_parse_u64(value, &body)) return 0;
    }
    return bytes.len - head >= body ? head + (size_t)body : 0;
}
static bool s_read_request(struct s2n_connection *conn, uint8_t *out, size_t capacity, size_t *length) {
    s2n_blocked_status blocked;
    size_t received = 0;
    while (received < capacity) {
        ssize_t count = s2n_recv(conn, out + received, capacity - received, &blocked);
        if (count <= 0) return false;
        received += (size_t)count;
        *length = s_request_length((struct aws_byte_cursor){ .len = received, .ptr = out });
        if (*length) return true;
    }
    return false;
}
static bool s_write(struct s2n_connection *conn, struct aws_byte_cursor bytes) {
    s2n_blocked_status blocked;
    while (bytes.len) {
        ssize_t count = s2n_send(conn, bytes.ptr, bytes.len, &blocked);
        if (count <= 0) return false;
        aws_byte_cursor_advance(&bytes, (size_t)count);
    }
    return true;
}
bool zaws_test_tls_serve(struct zaws_test_tls *tls, int fd, struct aws_byte_cursor response, uint8_t *out, size_t capacity, size_t *length) {
    struct s2n_connection *conn = s2n_connection_new(S2N_SERVER);
    s2n_blocked_status blocked;
    /* Self-service blinding: a failed handshake ends at once rather than after s2n's delay. */
    bool served = conn && s2n_connection_set_config(conn, tls->config) == S2N_SUCCESS &&
        s2n_connection_set_fd(conn, fd) == S2N_SUCCESS &&
        s2n_connection_set_blinding(conn, S2N_SELF_SERVICE_BLINDING) == S2N_SUCCESS &&
        s2n_negotiate(conn, &blocked) == S2N_SUCCESS && s_read_request(conn, out, capacity, length) &&
        s_write(conn, response);
    if (served) {
        uint8_t rest[256];
        while (s2n_recv(conn, rest, sizeof(rest), &blocked) > 0) {
        }
    }
    if (conn) s2n_connection_free(conn);
    return served;
}
