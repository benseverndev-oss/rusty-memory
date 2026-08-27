# The two-scenario comparison

Two conversations identical but for one sentence — one states there is no
partner, one never raises the subject — and the same question of both. A system
that answers them the same way cannot represent the difference.

Run it:

```sh
uv venv --python 3.12 && uv pip install --python .venv mem0ai
OPENAI_API_KEY=... .venv/Scripts/python.exe mem0_two_scenarios.py
```

`gpt-4o-mini` and `text-embedding-3-small` are pinned deliberately: they are the
models this project's own `rmem.toml` template names, so neither side is given a
better one. mem0's own default refused `temperature=0.1`, which is why the
config is explicit rather than left alone.

The result, and what it corrected, is in `docs/absence-benchmark.md`. The short
version: it disproved a claim this repository had made from reading mem0's
documentation rather than running it.

Nothing here is scored. The output is quoted verbatim, because the finding is
what each system says rather than a number derived from it.
