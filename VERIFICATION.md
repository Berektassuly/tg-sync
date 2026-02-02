# AI Analysis Feature - Verification Guide

## Prerequisites

- Rust toolchain installed
- API credentials in `.env` (TG_SYNC_API_ID, TG_SYNC_API_HASH)
- Authenticated Telegram session (run app once to login)
- At least one chat with messages synced

---

## Unit Tests

Run all unit tests:
```bash
cargo test
```

Run specific test modules:
```bash
# CSV generation tests
cargo test csv_utils

# JSON sanitization tests
cargo test openai_adapter

# SQL/repository tests
cargo test sqlite_repo
```

---

## Manual Verification Scenarios

### Scenario A: Mock Adapter (No API Key)

**Purpose:** Verify the pipeline works without real LLM costs.

1. **Ensure no API key is set:**
   ```bash
   $env:TG_SYNC_AI_API_KEY = ""  # PowerShell
   ```

2. **Run the application:**
   ```bash
   cargo run
   ```

3. **Select "AI Analysis" from menu.**

4. **Select a chat with synced messages.**

5. **Expected behavior:**
   - Warning: "TG_SYNC_AI_API_KEY not set, using mock AI adapter"
   - Spinner shows while "analyzing"
   - Report generated at `data/reports/analysis_<chat_id>_<week>.md`
   - Report contains `[MOCK]` placeholder text

6. ✅ **Pass criteria:** Report file exists, contains mock summary.

---

### Scenario B: Real OpenAI API

**Purpose:** Verify real LLM integration works.

1. **Set your API key:**
   ```bash
   $env:TG_SYNC_AI_API_KEY = "sk-..."  # PowerShell
   ```

2. **Run the application:**
   ```bash
   cargo run
   ```

3. **Select "AI Analysis" → select a chat.**

4. **Expected behavior:**
   - Log: "AI analysis enabled with OpenAI adapter"
   - Spinner shows during API call (may take 5-30 seconds)
   - Report generated with actual summary content
   - Summary references actual chat topics

5. ✅ **Pass criteria:** Report contains non-mock, contextual summary.

---

### Scenario C: Idempotency Check

**Purpose:** Verify re-running analysis skips already-analyzed weeks.

1. **Run analysis on a chat (Scenario A or B).**

2. **Note the report(s) generated.**

3. **Run analysis again on the SAME chat.**

4. **Expected behavior:**
   - Message: "No new weeks to analyze" OR ⏭️ icon
   - No new reports generated
   - Previously analyzed weeks are skipped

5. ✅ **Pass criteria:** Second run produces no new reports.

---

### Scenario D: Multiple Chats

**Purpose:** Verify batch processing with MultiSelect.

1. **Run the application, select "AI Analysis".**

2. **Use Space to select multiple chats, then Enter.**

3. **Expected behavior:**
   - Each chat shows its own spinner
   - Individual success/failure messages per chat
   - Total reports count at the end

4. ✅ **Pass criteria:** All selected chats processed independently.

---

## Database Verification

Check analysis_log table directly:
```bash
sqlite3 data/messages.db "SELECT chat_id, week_group, analyzed_at FROM analysis_log;"
```

Check generated reports:
```bash
dir data\reports\
```

---

## Troubleshooting

| Issue | Cause | Solution |
|-------|-------|----------|
| "No dialogs found" | Not authenticated | Run app, complete login flow first |
| "No unanalyzed weeks" | Already analyzed | Check `analysis_log` table, or test with different chat |
| API timeout | Slow LLM response | Increase timeout or use local Ollama |
| JSON parse error | LLM returned invalid format | Check logs, report may contain markdown wrapper |
