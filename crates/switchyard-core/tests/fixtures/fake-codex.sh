#!/bin/sh
# A stand-in for `codex app-server` (no model, no network): logs its arguments, working
# directory and every line it is sent to $FAKE_CODEX_LOG, and answers the handshake and one
# turn with canned JSON-RPC. A turn whose prompt says "ask" first asks to run a command.
log="${FAKE_CODEX_LOG:?}"
echo "args=$*" >> "$log"
echo "cwd=$(pwd -P)" >> "$log"
while IFS= read -r line; do
  echo "in=$line" >> "$log"
  case "$line" in
    *'"method":"initialize"'*)
      echo '{"id":1,"result":{"userAgent":"switchyard/0.144.1 (fake)","codexHome":"/tmp","platformFamily":"unix","platformOs":"macos"}}' ;;
    *'"method":"thread/start"'*)
      echo '{"id":2,"result":{"thread":{"id":"thread-fake","cliVersion":"0.144.1"},"model":"gpt-5.6-terra","approvalPolicy":"untrusted","sandbox":{"type":"workspaceWrite","writableRoots":[],"networkAccess":false}}}' ;;
    *'"method":"turn/start"'*'ask'*)
      echo '{"id":3,"result":{"turn":{"id":"turn-1","status":"inProgress"}}}'
      echo '{"method":"item/commandExecution/requestApproval","id":0,"params":{"threadId":"thread-fake","turnId":"turn-1","itemId":"c1","command":"pnpm test","cwd":"/wt","reason":"Run the tests"}}' ;;
    *'"result":{"decision":"accept"}'*)
      echo '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"a1","text":"Tests ran."}}}'
      echo '{"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}' ;;
    *'"method":"turn/start"'*)
      echo '{"id":3,"result":{"turn":{"id":"turn-1","status":"inProgress"}}}'
      echo '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"a1","text":"Hello from Codex."}}}'
      echo '{"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}' ;;
  esac
done
