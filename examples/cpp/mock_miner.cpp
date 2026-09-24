// A complete mock Quip solver in C++.
//
// It implements the whole solver contract — the four command-line modes, the
// handshake, credits, cancellation, and the exit codes — by registering one
// sampling callback with libquip_solver_c and letting the Rust session loop
// drive everything else. SPEC section 9 calls for exactly this: wrap the one
// loop, do not reimplement it.
//
// The sampler itself is deliberately trivial: it returns `num_reads` copies of
// the all-(+1) configuration, scored with the consensus scorer so the reported
// energy is the one the network will recompute.

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

#include "quip_solver.h"

namespace {

// Per-solver state. Reached through the void* the session loop threads back to
// every callback, so a real backend keeps its device handle here.
struct MockDevice {
    std::uint64_t jobs_sampled = 0;
};

// Called once per job by the Rust session loop, on a blocking worker thread.
//
// Returns QUIP_SAMPLE_OK after emitting solutions, or a QUIP_SAMPLE_* code.
// It must not throw: unwinding across the C ABI into Rust is undefined, so the
// whole body is wrapped.
std::int32_t sample(void* user_data,
                    const QuipIsingGraph* graph,
                    const QuipSampleParams* params,
                    QuipEmitFn emit,
                    void* sink) {
    try {
        auto* device = static_cast<MockDevice*>(user_data);
        if (graph == nullptr || params == nullptr) {
            return QUIP_SAMPLE_DEVICE_FAULT;
        }

        // A real backend would refuse a problem larger than its hardware. This
        // mirrors that so the Capacity -> RejectReason::TooLarge path has a
        // producer at all.
        //
        // In session mode it stays dead: the session validates a job against
        // the advertised max_nodes before it ever calls this callback, so a
        // graph this large is rejected upstream. The reachable route is
        // --solve, which reads a problem straight from stdin and hands it over
        // without that bound check.
        if (graph->num_nodes > 100000) {
            return QUIP_SAMPLE_CAPACITY;
        }

        const std::vector<std::int8_t> spins(graph->num_nodes, 1);

        // Score with the shipped consensus scorer, never a local reimplementation.
        //
        // The energy arrives through an out-parameter because 0 is a perfectly
        // legal energy: only the status distinguishes a scored problem from a
        // rejected one. Reporting an energy this call never produced would put
        // a wrong score on the wire.
        std::int64_t energy = 0;
        const std::int32_t scored = quip_energy_milli(graph->h,
                                                      graph->num_nodes,
                                                      graph->j,
                                                      graph->num_edges,
                                                      graph->edges,
                                                      spins.data(),
                                                      spins.size(),
                                                      &energy);
        if (scored != QUIP_ENERGY_OK) {
            std::fprintf(stderr,
                         "mock-cpp: cannot score this problem (status %d)\n",
                         scored);
            return QUIP_SAMPLE_DEVICE_FAULT;
        }

        for (std::size_t read = 0; read < params->num_reads; ++read) {
            emit(sink, spins.data(), spins.size(), energy);
        }
        device->jobs_sampled += 1;
        return QUIP_SAMPLE_OK;
    } catch (...) {
        // A device that threw is a device that needs a restart.
        return QUIP_SAMPLE_DEVICE_FAULT;
    }
}

}  // namespace

int main(int argc, char** argv) {
    MockDevice device;

    QuipBackendIdentity id;
    std::memset(&id, 0, sizeof(id));
    id.backend = "mock";
    id.algorithm = "sa";
    id.max_nodes = 100000;
    id.max_edges = 1000000;
    id.features = nullptr;
    id.num_features = 0;
    id.min_sweeps = 64;
    id.max_sweeps = 4096;
    id.min_reads = 64;
    id.max_reads = 512;
    id.reads_solution_min_factor = 4;
    id.reads_solution_max_factor = 8;
    id.reads_solution_floor_factor = 0;

    // quip_solver_run parses the normative command line itself, so --capabilities,
    // --solve, --check and session mode all work without any argument handling here.
    return quip_solver_run(&id, argc, argv, sample, &device);
}
