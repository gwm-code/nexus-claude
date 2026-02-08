#!/usr/bin/env python3
"""OpenClaw → Nexus Memory Migration Tool"""

import sqlite3
import json
import os
from pathlib import Path
from datetime import datetime

def main():
    openclaw_path = Path.home() / ".openclaw"
    nexus_path = Path.home() / ".config" / "nexus"

    print("🔄 OpenClaw → Nexus Migration Tool")
    print("━" * 40)
    print()

    # Create directories
    (nexus_path / "memory" / "vector").mkdir(parents=True, exist_ok=True)
    (nexus_path / "memory" / "events").mkdir(parents=True, exist_ok=True)
    (nexus_path / "memory" / "graph").mkdir(parents=True, exist_ok=True)

    # Step 1: Migrate code chunks
    print("📦 Step 1: Migrating code chunks & embeddings...")
    migrated_chunks = migrate_code_chunks(openclaw_path, nexus_path)
    print(f"   ✅ Migrated {migrated_chunks} chunks")

    # Step 2: Migrate conversations
    print("\n💬 Step 2: Migrating conversation history...")
    migrated_messages = migrate_conversations(openclaw_path, nexus_path)
    print(f"   ✅ Migrated {migrated_messages} messages")

    # Step 3: Extract personality
    print("\n🧠 Step 3: Extracting agent personality...")
    extract_personality(openclaw_path, nexus_path)
    print("   ✅ Extracted personality config")

    # Summary
    print("\n📊 Migration Summary")
    print("━" * 40)
    print(f"✅ Migrated {migrated_chunks} code chunks")
    print(f"✅ Migrated {migrated_messages} conversation messages")
    print(f"✅ Extracted personality config")
    print()
    print(f"Location: {nexus_path / 'memory'}")
    print("\nNext steps:")
    print("1. Review migrated data")
    print("2. Run: nexus memory-stats")

def migrate_code_chunks(openclaw_path, nexus_path):
    db_path = openclaw_path / "memory" / "main.sqlite"
    if not db_path.exists():
        print("   ⚠️  No OpenClaw database found")
        return 0

    conn = sqlite3.connect(str(db_path))
    cursor = conn.cursor()

    cursor.execute("SELECT COUNT(*) FROM chunks")
    count = cursor.fetchone()[0]
    print(f"   Found {count} code chunks")

    cursor.execute("""
        SELECT id, path, source, start_line, end_line, hash, model, text, embedding, updated_at
        FROM chunks
        ORDER BY path, start_line
    """)

    migrated = 0
    for row in cursor.fetchall():
        chunk_id, path, source, start_line, end_line, hash_val, model, text, embedding, updated_at = row

        try:
            embedding_list = json.loads(embedding) if embedding else []
        except:
            embedding_list = []

        if not embedding_list:
            continue

        entry = {
            "id": chunk_id,
            "content": text,
            "metadata": {
                "source_file": path,
                "start_line": start_line,
                "end_line": end_line,
                "hash": hash_val,
                "model": model,
                "source": source,
                "migrated_from": "openclaw",
                "original_timestamp": updated_at,
            },
            "embedding": embedding_list,
            "timestamp": updated_at,
        }

        output_file = nexus_path / "memory" / "vector" / f"{chunk_id}.json"
        with open(output_file, 'w') as f:
            json.dump(entry, f, indent=2)

        migrated += 1

    conn.close()
    return migrated

def migrate_conversations(openclaw_path, nexus_path):
    sessions_path = openclaw_path / "agents" / "main" / "sessions"
    if not sessions_path.exists():
        print("   ⚠️  No sessions found")
        return 0

    sessions = [
        f for f in sessions_path.glob("*.jsonl")
        if ".deleted." not in f.name
    ]
    print(f"   Found {len(sessions)} active sessions")

    total_messages = 0
    for session_file in sessions:
        session_id = session_file.stem

        with open(session_file, 'r') as f:
            for idx, line in enumerate(f):
                try:
                    msg = json.loads(line)
                except:
                    continue

                # OpenClaw format: {"type":"message","message":{"role":"...","content":[{"type":"text","text":"..."}]}}
                if msg.get("type") != "message" or "message" not in msg:
                    continue

                inner_msg = msg["message"]
                if 'role' not in inner_msg or 'content' not in inner_msg:
                    continue

                # Extract text from content array
                content_parts = inner_msg.get("content", [])
                if isinstance(content_parts, list):
                    text_content = " ".join([
                        part.get("text", "") for part in content_parts
                        if isinstance(part, dict) and part.get("type") == "text"
                    ])
                else:
                    text_content = str(content_parts)

                event = {
                    "type": "conversation",
                    "role": inner_msg.get("role"),
                    "content": text_content,
                    "timestamp": msg.get("timestamp", "unknown"),
                    "session": session_id,
                    "message_index": idx,
                    "migrated_from": "openclaw",
                }

                event_file = nexus_path / "memory" / "events" / f"openclaw_{session_id}_{idx}.json"
                with open(event_file, 'w') as f:
                    json.dump(event, f, indent=2)

                total_messages += 1

    return total_messages

def extract_personality(openclaw_path, nexus_path):
    config_path = openclaw_path / "openclaw.json"
    if not config_path.exists():
        print("   ⚠️  No config found")
        return

    with open(config_path, 'r') as f:
        config = json.load(f)

    personality = {
        "source": "openclaw",
        "migrated_at": datetime.utcnow().isoformat() + "Z",
        "config": config,
        "notes": "Migrated from OpenClaw. Review and integrate into Nexus system prompts.",
    }

    personality_file = nexus_path / "memory" / "openclaw_personality.json"
    with open(personality_file, 'w') as f:
        json.dump(personality, f, indent=2)

if __name__ == "__main__":
    main()
