/**
 * Built-in CostSink implementations: in-memory, local-file, Supabase.
 */

import { readFile } from 'node:fs/promises';
import { exchange, jsonArray } from './http.js';
import { replaceFile } from './persistence.js';
import type { CostRecord, CostSink, BudgetStatus, BudgetPeriod } from '../types.js';

export class MemorySink implements CostSink {
  records: CostRecord[] = [];
  async write(records: CostRecord[]): Promise<void> {
    for (const r of records) this.records.push(r);
  }
  async read(): Promise<CostRecord[]> { return this.records.slice(); }
}

export class FileSink implements CostSink {
  constructor(private path: string) {}
  async write(records: CostRecord[]): Promise<void> {
    const existing = await this.read();
    for (const record of records) existing.push(record);
    await replaceFile(this.path, JSON.stringify(existing, null, 2));
  }
  async read(): Promise<CostRecord[]> {
    let contents: string;
    try {
      contents = await readFile(this.path, 'utf8');
    } catch (cause) {
      if ((cause as NodeJS.ErrnoException).code === 'ENOENT') return [];
      throw new Error(`FileSink read ${this.path} failed: ${cause instanceof Error ? cause.message : String(cause)}`, { cause });
    }
    try {
      const records: unknown = JSON.parse(contents);
      if (!Array.isArray(records)) throw new Error('expected a JSON array');
      return records as CostRecord[];
    } catch (cause) {
      throw new Error(`FileSink cannot parse ${this.path}: ${cause instanceof Error ? cause.message : String(cause)}`, { cause });
    }
  }
}

export interface SupabaseSinkOptions {
  url: string;
  key: string;          // service-role key
  table?: string;       // default 'cost_records'
  budgetTable?: string; // default 'cost_budgets'
  viewName?: string;    // default 'cost_budget_status'
}

/** Supabase REST sink. Writes via PostgREST so we don't need the JS client
 *  as a dependency — keeps the package zero-runtime-dep. */
export class SupabaseSink implements CostSink {
  private url: string;
  private key: string;
  private table: string;
  private budgetTable: string;
  private viewName: string;
  constructor(opts: SupabaseSinkOptions) {
    this.url = opts.url.replace(/\/$/, '');
    this.key = opts.key;
    this.table = opts.table ?? 'cost_records';
    this.budgetTable = opts.budgetTable ?? 'cost_budgets';
    this.viewName = opts.viewName ?? 'cost_budget_status';
  }

  private resourceUrl(resource: string): URL {
    return new URL(`${this.url}/rest/v1/${encodeURIComponent(resource)}`);
  }

  private headers(prefer = 'return=minimal'): Record<string, string> {
    return {
      apikey: this.key,
      Authorization: `Bearer ${this.key}`,
      'Content-Type': 'application/json',
      Prefer: prefer,
    };
  }

  private async pages<T>(url: URL, operation: string): Promise<T[]> {
    const rows: T[] = [];
    for (;;) {
      const offset = rows.length;
      url.searchParams.set('offset', String(offset));
      const progress = `${operation} after ${offset} rows`;
      const reply = await exchange(url, progress, 'GET', this.headers('return=representation,count=exact'));
      const page = jsonArray<T>(reply, progress, url);
      const range = reply.headers['content-range'];
      const count = typeof range === 'string' ? Number(range.split('/').at(-1)) : NaN;
      const total = Number.isSafeInteger(count) && count >= 0 ? count : undefined;
      if (page.length === 0) {
        if (total !== undefined && offset < total) {
          throw new Error(`SupabaseSink ${operation} stopped after ${offset} rows but Content-Range reports ${range}`);
        }
        return rows;
      }
      for (const row of page) rows.push(row);
      if (total !== undefined && rows.length >= total) return rows;
    }
  }

  async write(records: CostRecord[]): Promise<void> {
    if (records.length === 0) return;
    await exchange(this.resourceUrl(this.table), 'write', 'POST', this.headers(), JSON.stringify(records));
  }

  async read(agent_id: string, since: Date): Promise<CostRecord[]> {
    const url = this.resourceUrl(this.table);
    url.searchParams.set('agent_id', `eq.${agent_id}`);
    url.searchParams.set('created_at', `gte.${since.toISOString()}`);
    url.searchParams.set('select', '*');
    url.searchParams.set('order', 'created_at.asc,id.asc');
    return this.pages<CostRecord>(url, 'read');
  }

  async readBudgets(agent_id: string): Promise<BudgetStatus[]> {
    const url = this.resourceUrl(this.viewName);
    url.searchParams.set('agent_id', `eq.${agent_id}`);
    url.searchParams.set('select', '*');
    url.searchParams.set('order', 'starts_at.asc,id.asc');
    const rows = await this.pages<Record<string, any>>(url, 'readBudgets');
    return rows.map(r => ({
      category: r.category,
      allocated_usd: Number(r.allocated_usd),
      spent_usd: Number(r.spent_usd),
      remaining_usd: Number(r.remaining_usd),
      utilization_pct: Number(r.utilization_pct),
      is_over_budget: !!r.is_over_budget,
      period: r.period as BudgetPeriod,
      starts_at: r.starts_at,
    }));
  }

  async writeBudget(agent_id: string, category: string, allocated_usd: number, period: BudgetPeriod, starts_at: Date): Promise<void> {
    const url = this.resourceUrl(this.budgetTable);
    url.searchParams.set('on_conflict', 'agent_id,category,period,starts_at');
    await exchange(url, 'writeBudget', 'POST', this.headers('resolution=merge-duplicates,return=minimal'),
      JSON.stringify({ agent_id, category, allocated_usd, period, starts_at: starts_at.toISOString() }));
  }
}
