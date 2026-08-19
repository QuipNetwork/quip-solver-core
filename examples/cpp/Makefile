# Builds the C++ mock miner against the Quip solver C library.
#
# QUIP_LIB_DIR must hold libquip_solver_c.so (or .a) and QUIP_INCLUDE_DIR must
# hold quip_solver.h. Both come from the release artifact; the defaults point at
# a local `cargo build -p quip-solver-c --release` inside this repository.

REPO_ROOT       ?= ../..
QUIP_LIB_DIR    ?= $(REPO_ROOT)/target/release
QUIP_INCLUDE_DIR?= $(REPO_ROOT)/crates/quip-solver-c/include

CXX      ?= g++
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra
LDFLAGS  ?= -L$(QUIP_LIB_DIR) -Wl,-rpath,$(abspath $(QUIP_LIB_DIR))
LDLIBS   ?= -lquip_solver_c -lpthread -ldl -lm

mock_miner: mock_miner.cpp
	$(CXX) $(CXXFLAGS) -I$(QUIP_INCLUDE_DIR) $< -o $@ $(LDFLAGS) $(LDLIBS)

# Builds the C library this miner links against.
.PHONY: lib
lib:
	cd $(REPO_ROOT) && cargo build -p quip-solver-c --release

# The conformance gate from SPEC section 6. Exit 0 means conformant.
.PHONY: conformance
conformance: mock_miner
	cd $(REPO_ROOT) && cargo build -p quip-solver-conformance --bin quip-solver-drive --release
	$(REPO_ROOT)/target/release/quip-solver-drive ./mock_miner unix:///tmp/quip-cpp-conformance.sock

.PHONY: clean
clean:
	rm -f mock_miner
