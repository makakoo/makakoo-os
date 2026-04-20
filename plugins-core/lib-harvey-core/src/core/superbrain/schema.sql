-- Harvey Superbrain — PostgreSQL Schema
-- Run against harvey_brain database (localhost:5434)
--
-- NOTE: Vector search (3072-dim) lives in Qdrant (collections: "multimodal", "brain")
-- PostgreSQL handles structured data only (no vector columns needed here)
-- This avoids pgvector 0.5.1 HNSW 2000-dim limit on PG16

-- ============================================================
-- Events (unified activity log from all agents)
-- ============================================================
CREATE TABLE IF NOT EXISTS events (
    id SERIAL PRIMARY KEY,
    event_type TEXT NOT NULL,
    agent TEXT NOT NULL,
    summary TEXT NOT NULL,
    details JSONB DEFAULT '{}',
    occurred_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS events_type_idx
    ON events (event_type, occurred_at DESC);

CREATE INDEX IF NOT EXISTS events_agent_idx
    ON events (agent, occurred_at DESC);

-- ============================================================
-- CRM Leads (structured career data)
-- ============================================================
CREATE TABLE IF NOT EXISTS crm_leads (
    id SERIAL PRIMARY KEY,
    company TEXT NOT NULL,
    contact_name TEXT,
    contact_email TEXT,
    role_title TEXT,
    status TEXT DEFAULT 'new',
    source TEXT,
    notes TEXT,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS crm_leads_status_idx
    ON crm_leads (status, updated_at DESC);

-- ============================================================
-- Trades (structured trading data)
-- ============================================================
CREATE TABLE IF NOT EXISTS trades (
    id SERIAL PRIMARY KEY,
    symbol TEXT NOT NULL,
    direction TEXT NOT NULL,
    entry_price NUMERIC,
    exit_price NUMERIC,
    quantity NUMERIC,
    pnl NUMERIC,
    strategy TEXT,
    opened_at TIMESTAMP,
    closed_at TIMESTAMP,
    metadata JSONB DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS trades_symbol_idx
    ON trades (symbol, closed_at DESC);

-- ============================================================
-- Ingestion Log (track what's been processed)
-- ============================================================
CREATE TABLE IF NOT EXISTS ingestion_log (
    id SERIAL PRIMARY KEY,
    source_path TEXT NOT NULL,
    source_type TEXT NOT NULL,
    target_system TEXT NOT NULL,
    status TEXT DEFAULT 'pending',
    content_hash TEXT,
    chunks_count INT DEFAULT 0,
    processed_at TIMESTAMP,
    error_msg TEXT
);

CREATE INDEX IF NOT EXISTS ingestion_log_status_idx
    ON ingestion_log (status, processed_at DESC);
