# Manim companion — visual chapters rendered; narration pending

Five concise chapters accompany the **45-minute total** presentation. Narration is part of the allocated sections, not extra runtime. The player stops at every chapter and waits for the presenter. It never pretends to monitor Pi or advances after a fixed silence. Use Pause at any time during a chapter. Model-generated diagrams/dots are labeled explanatory animation; recorded component tests stay separate.

Original visual language: dark teal field, mint capability edges, amber boundary/checkpoint annotations, progressive emphasis and a clear causal sequence. No 3Blue1Brown branding, character, music, or voice is used. Stock Phonon voice **Easton** only; no cloning or impersonation.

## Reproduce in an isolated environment

Install the system prerequisites for Manim Community on the chosen machine (Pango/Cairo development libraries, FFmpeg). Then:

```sh
cd demos/interview-walkthrough/animation
python3 -m venv .venv
.venv/bin/pip install -r requirements.lock
# Resolve and save one immutable model revision before downloading weights.
.venv/bin/python narrate.py --output output --resolve-model --sample
.venv/bin/python render.py --output output --sample
# Inspect sample timing, pronunciation, framing, RAM, disk and render time.
# Only after the sample passes, synthesize and render the five chapters:
.venv/bin/python narrate.py --output output
.venv/bin/python render.py --output output
```

The visual-only `--sample` is explicitly silent and 480p; it is not a narrated result. For a narrated one-beat sample, generate with `narrate.py --sample`, then run `render.py` **without** `--sample`; it renders exactly the narration manifest's single beat. The final full run overwrites the sample manifest with all chapters. Audio duration drives scene timing and Manim muxes the WAV into each MP4, so pause/resume controls both together.

`model-lock.json` records the Hugging Face commit, license and file inventory. `narration.json` records durations, WAV hashes and synthesis times. No main-branch model download proceeds without an immutable resolved revision. Cache/download is local; there is no paid inference or transmission of user data. Runtime is `ptts==0.4.1`, Manim Community `0.22.0`, NumPy `2.5.4`. The package pins are verified from PyPI; model revision resolution remains blocked in this cloud environment and must succeed on the rendering host.

Serve through `python3 ../demo.py serve` and use the slide companion button, or serve the animation directory using a local HTTP server. `player.html` reports missing media clearly. Do not commit model caches or full video build intermediates. Preserve generated hashes/provenance with any delivered media.

## Storyboard and timing

- 00–03: presenter opens and starts the alert investigation.
- 03–12: chapter 1, then Compose walkthrough while Pi works.
- 12–18: chapter 2, policy and actual dashboard/browser handoff.
- 18–28: chapter 3, paired conversation/filesystem fork and merge.
- 28–35: chapter 4, controlled voter failure and recovery.
- 35–39: chapter 5, evidence-based tradeoffs.
- 39–45: six minutes Q&A.

`storyboard.json` contains every spoken word, focus beat, source citation and checkpoint. Exact total narration duration is intentionally not asserted before synthesis; use the measured output manifest. Shorten narration if it crowds live work; keep the fixed 45-minute agenda.

## Actual verification and blockers

- `ptts==0.4.1` installed in an isolated cloud venv; package metadata reports MIT OR Apache-2.0. No model synthesis occurred.
- Model card identifies `gradium/phonon-31m-en`, Apache-2.0, English 24 kHz mono, q8 weights 64.5 MB excluding tokenizer. Direct model metadata access returned proxy **HTTP 403**. No proxy bypass, alternate voice, credentials or `trust_remote_code` was used.
- Initial Manim installation failed on missing Pango development metadata. This was resolved by extracting official Debian development packages into task scratch space and compiling the bindings there, without altering system libraries. Manim 0.22.0 installed successfully.
- The official `manimcommunity/manim:v0.22.0` image pull reached layer extraction but failed **no space left on device**. The failed image approach was abandoned. The primary demo source/state was preserved.
- Actual Manim sample: 5.8 seconds, 854×480, 89,151 bytes; 4.3 seconds render wall time, approximately 315 MiB peak process RSS. Five complete silent visual previews subsequently rendered, totaling 977,002 bytes. Frames were inspected for clipping/overlap. See previews/verification.json. Phonon model access and speech quality remain unverified; this is not a completed narrated result.

Sources: [Phonon model card](https://huggingface.co/gradium/phonon-31m-en), [official ptts package](https://pypi.org/project/ptts/0.4.1/), [Manim Community](https://pypi.org/project/manim/0.22.0/). Preserve Apache-2.0 model notices and MIT Manim notices when packaging rendered outputs/dependencies.

## Included visual previews

Use **Load silent Manim previews** in the companion player. These are actual Manim renders with no audio stream, not placeholder animations or live recordings. Each ends at a presenter-controlled checkpoint. `previews/verification.json` contains measured format, duration, and sample resource evidence. To regenerate all silent chapters: `python render.py --output output --silent-preview`. The narrated path uses the same scene code with measured Phonon audio duration.

## Exact remaining Phonon blocker and next steps

Target: `https://huggingface.co/api/models/gradium/phonon-31m-en`. Action: resolve the official model commit and file inventory before downloading weights. The configured cloud proxy returned `Tunnel connection failed: 403 Forbidden`; the request did not obtain model metadata. No model weights or voice audio were downloaded/generated. This is distinct from the model card, which was readable through the web research tool.

Permitted next step: on the connected Mac or another authorized host with ordinary Hugging Face access, run `narrate.py --output output --resolve-model --sample`, inspect the resolved Apache-2.0 revision and sample, then render that narration with `render.py --output output`. No public-model login should be assumed necessary. If access is still denied, stop and report it; do not change proxies, use mirrors/tunnels, request broad credentials, or substitute a voice. Existing silent previews remain labeled silent.
