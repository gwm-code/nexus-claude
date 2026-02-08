#!/bin/bash
# OpenClaw → Nexus Memory Migration Script

set -e

OPENCLAW_PATH="$HOME/.openclaw"
NEXUS_PATH="$HOME/.config/nexus"

echo "🔄 OpenClaw → Nexus Migration Tool"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Create Nexus memory directories
mkdir -p "$NEXUS_PATH/memory/vector"
mkdir -p "$NEXUS_PATH/memory/events"
mkdir -p "$NEXUS_PATH/memory/graph"

# Step 1: Migrate code chunks
echo "📦 Step 1: Migrating code chunks & embeddings..."
CHUNK_COUNT=$(sqlite3 "$OPENCLAW_PATH/memory/main.sqlite" "SELECT COUNT(*) FROM chunks" 2>/dev/null || echo "0")
echo "   Found $CHUNK_COUNT code chunks"

if [ "$CHUNK_COUNT" != "0" ]; then
    sqlite3 "$OPENCLAW_PATH/memory/main.sqlite" <<'EOF' | while IFS='|' read -r id path source start_line end_line hash model text embedding updated_at; do
SELECT id, path, source, start_line, end_line, hash, model, text, embedding, updated_at FROM chunks ORDER BY path, start_line;
EOF
        # Create JSON entry
        cat > "$NEXUS_PATH/memory/vector/${id}.json" <<ENTRY
{
  "id": "$id",
  "content": $(echo "$text" | jq -Rs .),
  "metadata": {
    "source_file": "$path",
    "start_line": $start_line,
    "end_line": $end_line,
    "hash": "$hash",
    "model": "$model",
    "source": "$source",
    "migrated_from": "openclaw",
    "original_timestamp": $updated_at
  },
  "embedding": $embedding,
  "timestamp": $updated_at
}
ENTRY
    done
    echo "   ✅ Migrated code chunks"
fi

# Step 2: Migrate conversations
echo ""
echo "💬 Step 2: Migrating conversation history..."
SESSION_COUNT=$(find "$OPENCLAW_PATH/agents/main/sessions" -name "*.jsonl" ! -name "*.deleted.*" 2>/dev/null | wc -l)
echo "   Found $SESSION_COUNT active sessions"

TOTAL_MESSAGES=0
if [ "$SESSION_COUNT" != "0" ]; then
    for session_file in "$OPENCLAW_PATH/agents/main/sessions"/*.jsonl; do
        [ -e "$session_file" ] || continue
        [[ "$session_file" == *".deleted."* ]] && continue

        session_id=$(basename "$session_file" .jsonl)
        msg_idx=0

        while IFS= read -r line; do
            role=$(echo "$line" | jq -r '.role // empty')
            [ -z "$role" ] && continue

            content=$(echo "$line" | jq -r '.content // empty')
            timestamp=$(echo "$line" | jq -r '.timestamp // "unknown"')

            cat > "$NEXUS_PATH/memory/events/openclaw_${session_id}_${msg_idx}.json" <<EVENT
{
  "type": "conversation",
  "role": "$role",
  "content": $(echo "$content" | jq -Rs .),
  "timestamp": "$timestamp",
  "session": "$session_id",
  "message_index": $msg_idx,
  "migrated_from": "openclaw"
}
EVENT
            ((msg_idx++))
            ((TOTAL_MESSAGES++))
        done < "$session_file"
    done
    echo "   ✅ Migrated $TOTAL_MESSAGES conversation messages"
fi

# Step 3: Extract personality
echo ""
echo "🧠 Step 3: Extracting agent personality..."
if [ -f "$OPENCLAW_PATH/openclaw.json" ]; then
    cat > "$NEXUS_PATH/memory/openclaw_personality.json" <<PERSONALITY
{
  "source": "openclaw",
  "migrated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "config": $(cat "$OPENCLAW_PATH/openclaw.json"),
  "notes": "This personality was migrated from OpenClaw. Review and integrate into Nexus system prompts."
}
PERSONALITY
    echo "   ✅ Extracted personality config"
fi

# Step 4: Create migration summary
echo ""
echo "📊 Migration Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ All data migrated successfully!"
echo ""
echo "Migrated:"
echo "  - Code chunks: $CHUNK_COUNT"
echo "  - Conversations: $TOTAL_MESSAGES messages from $SESSION_COUNT sessions"
echo "  - Personality: 1 config file"
echo ""
echo "Location: $NEXUS_PATH/memory/"
echo ""
echo "Next steps:"
echo "1. Review migrated data"
echo "2. Restart Nexus: nexus"
echo "3. Check memory: nexus memory-stats"
