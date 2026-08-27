"""Can a memory tell a stated absence from a silence?

Two conversations, identical in every way except one sentence:

  A. the speaker states there is no partner
  B. partners never come up at all

Then the same question of both. A system that answers them the same way
cannot represent the difference, which is the claim under test. Nothing here
is scored -- the output is quoted verbatim, because the finding is what each
system says, not a number derived from it.

Synthetic throughout: no real person appears.
"""
import json
import os
import sys

from mem0 import Memory

SHARED = [
    {"role": "user", "content": "I moved to Northwind Analytics last spring."},
    {"role": "assistant", "content": "Noted -- Northwind Analytics since last spring."},
    {"role": "user", "content": "I work out of the Leeds office, mostly on the data platform."},
    {"role": "assistant", "content": "Got it."},
]

STATES_ABSENCE = SHARED + [
    {"role": "user", "content": "I'm not married and I don't have a partner."},
    {"role": "assistant", "content": "Understood."},
]

NEVER_MENTIONED = SHARED + [
    {"role": "user", "content": "I cycle in most days when the weather holds."},
    {"role": "assistant", "content": "Understood."},
]

QUESTION = "Does this person have a partner?"


# gpt-4o-mini deliberately: it is the model rusty-memory's own template names,
# so both systems get the same one. mem0's default refused temperature=0.1.
CONFIG = {
    "llm": {"provider": "openai", "config": {"model": "gpt-4o-mini", "temperature": 0.1}},
    "embedder": {
        "provider": "openai",
        "config": {"model": "text-embedding-3-small"},
    },
}


def run(label, messages, user_id):
    m = Memory.from_config(CONFIG)
    m.add(messages, user_id=user_id)
    hits = m.search(QUESTION, filters={"user_id": user_id})
    results = hits.get("results", hits) if isinstance(hits, dict) else hits

    print(f"\n=== {label}")
    print(f"    asked: {QUESTION}")
    if not results:
        print("    returned: (nothing)")
    for r in results:
        text = r.get("memory") if isinstance(r, dict) else str(r)
        score = r.get("score") if isinstance(r, dict) else None
        print(f"    returned: {text!r}" + (f"  score={score:.4f}" if score else ""))
    return [r.get("memory") if isinstance(r, dict) else str(r) for r in results]


if __name__ == "__main__":
    if not os.environ.get("OPENAI_API_KEY"):
        sys.exit("OPENAI_API_KEY is not in the environment")

    a = run("A -- the conversation states there is no partner", STATES_ABSENCE, "subject-a")
    b = run("B -- partners are never mentioned", NEVER_MENTIONED, "subject-b")

    print("\n=== verdict")
    print("    A and B returned the same thing" if a == b
          else "    A and B returned different things")
    json.dump({"a": a, "b": b}, open("mem0-result.json", "w"), indent=2)
