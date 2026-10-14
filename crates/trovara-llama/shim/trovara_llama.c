// The few llama.cpp / mtmd calls that take or return structs by value, or need a parameter struct filled in. Compiled
// against the real headers (vendor/llama/include) by build.rs, so the C compiler gets every layout right; Rust calls
// only these plain functions and the plain ones in the headers (see src/ffi.rs).

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "llama.h"
#include "mtmd.h"
#include "mtmd-helper.h"

#include <ctype.h>

static bool contains_ignoring_case(const char * text, const char * part) {
    if (!text || !part || !*part) {
        return false;
    }
    for (; *text; text++) {
        size_t i = 0;
        while (part[i] && text[i] && tolower((unsigned char) text[i]) == tolower((unsigned char) part[i])) {
            i++;
        }
        if (!part[i]) {
            return true;
        }
    }
    return false;
}

// The one GPU the model runs on, or NULL for the CPU. `preference`: NULL or "" chooses (a discrete GPU before an
// integrated one, then the one with the most memory); "cpu" chooses the CPU; anything else picks the first GPU whose
// name or description contains it ("nvidia", "Vulkan1"). Splitting one small model across two GPUs (an iGPU and a
// discrete one, as most laptops have) only adds copies between them.
ggml_backend_dev_t trv_pick_device(const char * preference) {
    if (preference && contains_ignoring_case(preference, "cpu") && strlen(preference) == 3) {
        return NULL;
    }
    ggml_backend_dev_t best = NULL;
    int best_rank = -1;
    size_t best_memory = 0;
    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
        ggml_backend_dev_t device = ggml_backend_dev_get(i);
        enum ggml_backend_dev_type type = ggml_backend_dev_type(device);
        if (type != GGML_BACKEND_DEVICE_TYPE_GPU && type != GGML_BACKEND_DEVICE_TYPE_IGPU) {
            continue;
        }
        if (preference && *preference) {
            if (contains_ignoring_case(ggml_backend_dev_name(device), preference)
                || contains_ignoring_case(ggml_backend_dev_description(device), preference)) {
                return device;
            }
            continue;
        }
        size_t free = 0, total = 0;
        ggml_backend_dev_memory(device, &free, &total);
        int rank = type == GGML_BACKEND_DEVICE_TYPE_GPU ? 1 : 0;
        if (rank > best_rank || (rank == best_rank && total > best_memory)) {
            best = device;
            best_rank = rank;
            best_memory = total;
        }
    }
    return best;
}

// The whole model on `device` (no split), or on the CPU when it's NULL.
struct llama_model * trv_model_load(const char * path, ggml_backend_dev_t device) {
    ggml_backend_dev_t devices[2] = { device, NULL };
    struct llama_model_params params = llama_model_default_params();
    if (device) {
        params.devices = devices;   // read during the load only
        params.split_mode = LLAMA_SPLIT_MODE_NONE;
        params.main_gpu = 0;
        params.n_gpu_layers = 999;
    } else {
        params.n_gpu_layers = 0;
    }
    return llama_model_load_from_file(path, params);
}

// One input must fit one micro-batch (n_ctx = n_batch = n_ubatch); each input is its own sequence and may use the
// whole context (kv_unified), as llama-embedding does. Mean pooling, embeddings out.
struct llama_context * trv_context_init(struct llama_model * model, uint32_t max_tokens, uint32_t max_sequences,
                                        int32_t threads) {
    struct llama_context_params params = llama_context_default_params();
    params.n_ctx = max_tokens;
    params.n_batch = max_tokens;
    params.n_ubatch = max_tokens;
    params.n_seq_max = max_sequences;
    params.kv_unified = true;
    params.embeddings = true;
    params.pooling_type = LLAMA_POOLING_TYPE_MEAN;
    if (threads > 0) {
        params.n_threads = threads;
        params.n_threads_batch = threads;
    }
    return llama_init_from_model(model, params);
}

// Images arrive at their final size (src/image_sizing.rs): min 1 token so mtmd never rescales them.
// The encoders on the same device as the text model (NULL: the CPU).
mtmd_context * trv_mtmd_init(const char * path, const struct llama_model * model, int32_t threads,
                             ggml_backend_dev_t device) {
    struct mtmd_context_params params = mtmd_context_params_default();
    params.use_gpu = device != NULL;
    params.device = device;
    params.print_timings = false;
    params.warmup = true;
    params.image_min_tokens = 1;
    params.image_max_tokens = 1120;
    if (threads > 0) {
        params.n_threads = threads;
    }
    return mtmd_init_from_file(path, model, params);
}

int32_t trv_batch_add_embd(struct llama_batch_ext * batch, llama_seq_id seq, const float * row, size_t n_embd) {
    struct llama_embd embd = { row, 1, n_embd };   // copied by the batch
    return llama_batch_ext_add_embd(batch, seq, embd);
}

// An image or audio file's bytes, decoded by mtmd's helpers (stb_image; miniaudio at the encoder's sample rate).
// From bytes, not a path: on Windows its fopen would read the path in the ANSI code page. *is_video is set (and NULL
// returned) for a video, which goes in as frames and sound instead.
mtmd_bitmap * trv_bitmap_from_buf(mtmd_context * ctx, const unsigned char * buf, size_t len, bool * is_video) {
    struct mtmd_helper_bitmap_wrapper wrapper =
        mtmd_helper_bitmap_init_from_buf(ctx, buf, len, false, mtmd_helper_init_opt_default());
    *is_video = wrapper.video_ctx != NULL;
    if (wrapper.video_ctx) {
        if (wrapper.bitmap) {
            mtmd_bitmap_free(wrapper.bitmap);
        }
        mtmd_helper_video_free(wrapper.video_ctx);
        return NULL;
    }
    return wrapper.bitmap;
}

// `<bos> <media marker> <eos>` with one bitmap, as the reference frames media inputs.
int32_t trv_tokenize_media(mtmd_context * ctx, mtmd_input_chunks * chunks, const mtmd_bitmap * bitmap) {
    const char * marker = mtmd_default_marker();
    mtmd_input_text text;
    text.text = marker;
    text.text_len = strlen(marker);
    text.add_special = true;
    text.parse_special = true;
    const mtmd_bitmap * bitmaps[1] = { bitmap };
    return mtmd_tokenize(ctx, chunks, &text, bitmaps, 1);
}
