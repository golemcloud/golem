"""Posts one Main validation report to the Slack webhook in SLACK_BENCHMARK_WEBHOOK_URL.

Usage: python3 -I post-slack.py <run_id> <attempt> <kind> <message-file>
kind: failure | repeat-failure | recovery | test

The receipt receipts/<run_id>-a<attempt>.json is created exclusively before the POST. If it
already exists, nothing is sent: a pending or unknown receipt means delivery is uncertain and
needs a human decision, never an automatic retry. Delivery is confirmed only by HTTP 200 with
body "ok". The webhook value is never printed or stored.
"""
import datetime
import json
import os
import pathlib
import sys
import urllib.error
import urllib.parse
import urllib.request

run_id, attempt, kind, message_file = sys.argv[1:5]
root = pathlib.Path(__file__).resolve().parents[3]
state = pathlib.Path(os.environ.get("OBSERVER_STATE_DIR") or root / ".local/ci-observer")
receipt = state / "receipts" / f"{run_id}-a{attempt}.json"
message = pathlib.Path(message_file).read_text(encoding="utf-8").strip()
dashboard = os.environ.get("CI_DASHBOARD_URL", "").rstrip("/")
parsed = urllib.parse.urlsplit(dashboard)
if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password or parsed.query or parsed.fragment:
    sys.exit("CI_DASHBOARD_URL must be configured as a public HTTPS URL")
message += f"\n\n<{dashboard}/#run-{run_id}-a{attempt}|Dashboard>"
url = os.environ.get("SLACK_BENCHMARK_WEBHOOK_URL", "")
if not url:
    sys.exit("SLACK_BENCHMARK_WEBHOOK_URL is not set; nothing sent")


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


record = {"run_id": run_id, "attempt": attempt, "kind": kind, "created": now(), "delivery": "pending", "message": message}
receipt.parent.mkdir(parents=True, exist_ok=True)
try:
    with receipt.open("x", encoding="utf-8") as handle:
        json.dump(record, handle, indent=1)
except FileExistsError:
    existing = json.loads(receipt.read_text(encoding="utf-8"))
    sys.exit(f"receipt exists with delivery={existing.get('delivery')}; not sending again")

request = urllib.request.Request(
    url,
    data=json.dumps({"text": message, "mrkdwn": True}).encode(),
    headers={"Content-Type": "application/json"},
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        status, body = response.status, response.read().decode(errors="replace")[:200]
    record.update(delivery="confirmed" if status == 200 and body == "ok" else "unknown", httpStatus=status, response=body)
except urllib.error.HTTPError as error:
    record.update(delivery="rejected", httpStatus=error.code, response=error.read().decode(errors="replace")[:200])
except Exception as error:  # noqa: BLE001
    record.update(delivery="unknown", errorType=type(error).__name__)
record["finished"] = now()
receipt.write_text(json.dumps(record, indent=1), encoding="utf-8")
print(json.dumps({key: value for key, value in record.items() if key != "message"}))
sys.exit(0 if record["delivery"] == "confirmed" else 1)
