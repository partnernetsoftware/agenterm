/*
 * Minimal nng PAIR ping-pong over ipc:// (UDS). No TCP.
 *
 * Usage:
 *   nng_pair_bench server <ipc_url>
 *   nng_pair_bench client <ipc_url> <n> <payload_len> [xor]
 *
 * Client: warm 1 untimed round-trip, then time n round-trips.
 * Default toy work on server: XOR each payload byte with 0xA5 (match mmap-lab).
 * Pass "echo" as 5th arg for plain echo (document misalignment in RESULTS).
 */

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <nng/nng.h>
#include <nng/protocol/pair0/pair.h>

enum { MAX_PAYLOAD = 4096 };

static uint64_t
now_ns(void)
{
	struct timespec ts;
	if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
		perror("clock_gettime");
		exit(1);
	}
	return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static int
cmp_u64(const void *a, const void *b)
{
	uint64_t x = *(const uint64_t *)a;
	uint64_t y = *(const uint64_t *)b;
	return (x > y) - (x < y);
}

static uint64_t
percentile(uint64_t *sorted, size_t n, double p)
{
	if (n == 0) {
		return 0;
	}
	size_t idx = (size_t)((n - 1) * p + 0.5);
	if (idx >= n) {
		idx = n - 1;
	}
	return sorted[idx];
}

static void
die_nng(const char *what, int rv)
{
	fprintf(stderr, "nng_pair_bench: %s: %s\n", what, nng_strerror(rv));
	exit(1);
}

static void
run_server(const char *url, int do_xor)
{
	nng_socket sock;
	int rv;

	if ((rv = nng_pair0_open(&sock)) != 0) {
		die_nng("pair0_open", rv);
	}
	if ((rv = nng_listen(sock, url, NULL, 0)) != 0) {
		die_nng("listen", rv);
	}
	fprintf(stderr, "nng_pair_bench: server listening %s xor=%d\n", url, do_xor);

	for (;;) {
		nng_msg *msg = NULL;
		if ((rv = nng_recvmsg(sock, &msg, 0)) != 0) {
			die_nng("recvmsg", rv);
		}
		size_t len = nng_msg_len(msg);
		uint8_t *body = nng_msg_body(msg);
		if (do_xor) {
			for (size_t i = 0; i < len; i++) {
				body[i] ^= 0xA5;
			}
		}
		if ((rv = nng_sendmsg(sock, msg, 0)) != 0) {
			nng_msg_free(msg);
			die_nng("sendmsg", rv);
		}
		/* ownership transferred on success */
	}
}

static void
run_client(const char *url, size_t n, size_t payload_len, int do_xor)
{
	nng_socket sock;
	int rv;
	uint8_t req[MAX_PAYLOAD];
	uint8_t expect[MAX_PAYLOAD];

	if (payload_len == 0 || payload_len > MAX_PAYLOAD) {
		fprintf(stderr, "nng_pair_bench: bad payload_len %zu\n", payload_len);
		exit(2);
	}
	/* Same 20B string as mmap-lab when len==20; otherwise fill pattern. */
	if (payload_len == 20) {
		memcpy(req, "hello-mmap-ephemeral", 20);
	} else {
		for (size_t i = 0; i < payload_len; i++) {
			req[i] = (uint8_t)('A' + (i % 26));
		}
	}
	memcpy(expect, req, payload_len);
	if (do_xor) {
		for (size_t i = 0; i < payload_len; i++) {
			expect[i] ^= 0xA5;
		}
	}

	if ((rv = nng_pair0_open(&sock)) != 0) {
		die_nng("pair0_open", rv);
	}
	/* Brief dial retries while server binds. */
	for (int attempt = 0; attempt < 50; attempt++) {
		rv = nng_dial(sock, url, NULL, 0);
		if (rv == 0) {
			break;
		}
		struct timespec ts = { .tv_sec = 0, .tv_nsec = 20 * 1000 * 1000 };
		nanosleep(&ts, NULL);
	}
	if (rv != 0) {
		die_nng("dial", rv);
	}

	uint64_t *samples = calloc(n, sizeof(uint64_t));
	if (samples == NULL) {
		perror("calloc");
		exit(1);
	}

	for (size_t i = 0; i < n + 1; i++) {
		int warm = (i == 0);
		uint64_t t0 = now_ns();

		nng_msg *msg = NULL;
		if ((rv = nng_msg_alloc(&msg, payload_len)) != 0) {
			die_nng("msg_alloc", rv);
		}
		memcpy(nng_msg_body(msg), req, payload_len);
		if ((rv = nng_sendmsg(sock, msg, 0)) != 0) {
			nng_msg_free(msg);
			die_nng("sendmsg", rv);
		}

		nng_msg *resp = NULL;
		if ((rv = nng_recvmsg(sock, &resp, 0)) != 0) {
			die_nng("recvmsg", rv);
		}
		uint64_t dt = now_ns() - t0;

		if (nng_msg_len(resp) != payload_len ||
		    memcmp(nng_msg_body(resp), expect, payload_len) != 0) {
			fprintf(stderr, "nng_pair_bench: bad response (len or bytes)\n");
			nng_msg_free(resp);
			exit(1);
		}
		nng_msg_free(resp);

		if (!warm) {
			samples[i - 1] = dt;
		}
	}

	qsort(samples, n, sizeof(uint64_t), cmp_u64);
	uint64_t sum = 0;
	for (size_t i = 0; i < n; i++) {
		sum += samples[i];
	}
	uint64_t mean = n ? sum / n : 0;
	uint64_t minv = n ? samples[0] : 0;
	uint64_t maxv = n ? samples[n - 1] : 0;
	uint64_t p50 = percentile(samples, n, 0.50);
	uint64_t p95 = percentile(samples, n, 0.95);

	printf(
	    "nng(pair,ipc,xor=%d): n=%zu payload=%zuB warm=1 "
	    "min=%.3fµs p50=%.3fµs p95=%.3fµs max=%.3fµs mean=%.3fµs\n",
	    do_xor, n, payload_len, minv / 1000.0, p50 / 1000.0, p95 / 1000.0,
	    maxv / 1000.0, mean / 1000.0);

	free(samples);
	nng_close(sock);
}

/* Wall-clock RPS: warm then time n sequential round-trips as one interval. */
static void
run_client_rps(const char *url, size_t n, size_t warm, size_t payload_len, int do_xor)
{
	nng_socket sock;
	int rv;
	uint8_t req[MAX_PAYLOAD];
	uint8_t expect[MAX_PAYLOAD];

	if (payload_len == 0 || payload_len > MAX_PAYLOAD) {
		fprintf(stderr, "nng_pair_bench: bad payload_len %zu\n", payload_len);
		exit(2);
	}
	if (payload_len == 20) {
		memcpy(req, "hello-mmap-ephemeral", 20);
	} else {
		for (size_t i = 0; i < payload_len; i++) {
			req[i] = (uint8_t)('A' + (i % 26));
		}
	}
	memcpy(expect, req, payload_len);
	if (do_xor) {
		for (size_t i = 0; i < payload_len; i++) {
			expect[i] ^= 0xA5;
		}
	}

	if ((rv = nng_pair0_open(&sock)) != 0) {
		die_nng("pair0_open", rv);
	}
	for (int attempt = 0; attempt < 50; attempt++) {
		rv = nng_dial(sock, url, NULL, 0);
		if (rv == 0) {
			break;
		}
		struct timespec ts = { .tv_sec = 0, .tv_nsec = 20 * 1000 * 1000 };
		nanosleep(&ts, NULL);
	}
	if (rv != 0) {
		die_nng("dial", rv);
	}

	uint64_t t0 = 0;
	for (size_t i = 0; i < warm + n; i++) {
		if (i == warm) {
			t0 = now_ns();
		}

		nng_msg *msg = NULL;
		if ((rv = nng_msg_alloc(&msg, payload_len)) != 0) {
			die_nng("msg_alloc", rv);
		}
		memcpy(nng_msg_body(msg), req, payload_len);
		if ((rv = nng_sendmsg(sock, msg, 0)) != 0) {
			nng_msg_free(msg);
			die_nng("sendmsg", rv);
		}
		nng_msg *resp = NULL;
		if ((rv = nng_recvmsg(sock, &resp, 0)) != 0) {
			die_nng("recvmsg", rv);
		}
		if (nng_msg_len(resp) != payload_len ||
		    memcmp(nng_msg_body(resp), expect, payload_len) != 0) {
			fprintf(stderr, "nng_pair_bench: bad response (len or bytes)\n");
			nng_msg_free(resp);
			exit(1);
		}
		nng_msg_free(resp);
	}

	{
		uint64_t elapsed = now_ns() - t0;
		double secs = elapsed / 1e9;
		if (secs < 1e-12) {
			secs = 1e-12;
		}
		double rps = (double)n / secs;
		printf(
		    "nng-rps(pair,ipc,xor=%d): n=%zu warm=%zu payload=%zuB "
		    "elapsed=%.6fs rps=%.0f\n",
		    do_xor, n, warm, payload_len, secs, rps);
	}

	nng_close(sock);
}

int
main(int argc, char **argv)
{
	if (argc < 3) {
		fprintf(stderr,
		    "usage:\n"
		    "  %s server <ipc_url> [xor|echo]\n"
		    "  %s client <ipc_url> <n> <payload_len> [xor|echo]\n"
		    "  %s rps <ipc_url> <n> <warm> <payload_len> [xor|echo]\n"
		    "url must be ipc://... (no tcp)\n",
		    argv[0], argv[0], argv[0]);
		return 2;
	}

	const char *mode = argv[1];
	const char *url = argv[2];
	if (strncmp(url, "ipc://", 6) != 0) {
		fprintf(stderr, "nng_pair_bench: refuse non-ipc URL (want ipc://)\n");
		return 2;
	}

	int do_xor = 1;
	if (strcmp(mode, "server") == 0) {
		if (argc >= 4 && strcmp(argv[3], "echo") == 0) {
			do_xor = 0;
		}
		run_server(url, do_xor);
		return 0;
	}
	if (strcmp(mode, "client") == 0) {
		if (argc < 5) {
			fprintf(stderr, "nng_pair_bench: client needs n and payload_len\n");
			return 2;
		}
		size_t n = (size_t)strtoull(argv[3], NULL, 10);
		size_t plen = (size_t)strtoull(argv[4], NULL, 10);
		if (argc >= 6 && strcmp(argv[5], "echo") == 0) {
			do_xor = 0;
		}
		run_client(url, n, plen, do_xor);
		return 0;
	}
	if (strcmp(mode, "rps") == 0) {
		if (argc < 6) {
			fprintf(stderr, "nng_pair_bench: rps needs n warm payload_len\n");
			return 2;
		}
		size_t n = (size_t)strtoull(argv[3], NULL, 10);
		size_t warm = (size_t)strtoull(argv[4], NULL, 10);
		size_t plen = (size_t)strtoull(argv[5], NULL, 10);
		if (argc >= 7 && strcmp(argv[6], "echo") == 0) {
			do_xor = 0;
		}
		run_client_rps(url, n, warm, plen, do_xor);
		return 0;
	}

	fprintf(stderr, "nng_pair_bench: unknown mode %s\n", mode);
	return 2;
}
