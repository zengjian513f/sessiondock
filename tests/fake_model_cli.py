#!/usr/bin/env python3
# run_validation: skip
"""Private, free CLI stand-in for the new-session model picker tests.

Every invocation appends its argv as one JSON line to
`SESSIONDOCK_TEST_ARGV_LOG`. `--version` answers like an installed CLI;
`models` prints `SESSIONDOCK_TEST_MODELS`
(`provider/model` lines, like `opencode models`); `api …` answers `{}` like
a successful OpenCode API call; Grok agent stdio advertises its models at
initialize without creating a session; anything else is an interactive CLI that
just stays alive.
"""
import json
import os
import sys
import time


def main():
    with open(os.environ["SESSIONDOCK_TEST_ARGV_LOG"], "a", encoding="utf-8") as log:
        log.write(json.dumps(sys.argv[1:]) + "\n")
    if sys.argv[1:] == ["--version"]:
        print("fake-model-cli 1.0")
        return 0
    if sys.argv[1:] == ["debug", "models", "--bundled"]:
        catalog = os.environ.get("SESSIONDOCK_TEST_CODEX_BUNDLED")
        if not catalog or not os.path.exists(catalog):
            return 1
        with open(catalog) as source:
            print(source.read())
        return 0
    if sys.argv[1:] == ["agent", "--no-leader", "stdio"]:
        request = json.loads(sys.stdin.readline())
        assert request["method"] == "initialize", request
        print(json.dumps({"jsonrpc":"2.0","id":request["id"],"result":{
            "protocolVersion":1,"_meta":{"modelState":{
                "currentModelId":"grok-fake","availableModels":[{
                    "modelId":"grok-fake","name":"Grok Fake","_meta":{
                        "supportsReasoningEffort":True,"reasoningEfforts":[
                            {"value":"high","default":True},
                            {"value":"low","default":False}]}}]}}}}),flush=True)
        return 0
    if sys.argv[1:2] == ["models"]:
        print(os.environ.get("SESSIONDOCK_TEST_MODELS", ""))
        return 0
    if sys.argv[1:2] == ["api"]:
        print("{}")
        return 0
    print("fake cli ready", flush=True)
    time.sleep(120)
    return 0


if __name__ == "__main__":
    sys.exit(main())
