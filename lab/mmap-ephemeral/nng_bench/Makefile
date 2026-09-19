# Lab-only nng PAIR ping-pong (ipc://).
NNG_PREFIX ?= $(shell brew --prefix nng 2>/dev/null)
CC = /usr/bin/clang
CFLAGS = -O3 -Wall -Wextra -I$(NNG_PREFIX)/include
LDFLAGS = -L$(NNG_PREFIX)/lib -lnng -Wl,-rpath,$(NNG_PREFIX)/lib

.PHONY: all clean

all: nng_pair_bench

nng_pair_bench: nng_pair_bench.c
	@test -n "$(NNG_PREFIX)" || (echo "nng not found; brew install nng" >&2; exit 1)
	$(CC) $(CFLAGS) -o $@ $< $(LDFLAGS)

clean:
	rm -f nng_pair_bench
