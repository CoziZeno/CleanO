import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type IpcError } from "@/ipc/client";
import { toIpcError } from "@/ipc/client";
import type { QueryRequest } from "@/ipc/types";
import type { FileRow } from "@/ipc/types";

const PAGE_SIZE = 200;

interface PagedQuery {
  total: number;
  bytes: number;
  loading: boolean;
  error: IpcError | null;
  getRow: (index: number) => FileRow | undefined;
  ensureRange: (start: number, end: number) => void;
}

/**
 * Windowed access to the scan index.
 *
 * The whole point of the architecture is that the index stays in Rust: the UI
 * asks for totals plus the pages it is actually showing. A 100k-file folder
 * therefore costs the same to display as a 100-file one.
 *
 * `generation` changes when a new scan lands, which invalidates every cached
 * page - ids are indices into a specific scan and mean nothing across scans.
 */
export function usePagedQuery(request: QueryRequest, generation: number): PagedQuery {
  const [total, setTotal] = useState(0);
  const [bytes, setBytes] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [pages, setPages] = useState<Map<number, FileRow[]>>(() => new Map());

  const inFlight = useRef<Set<number>>(new Set());
  // Guards against a slow response for an old filter overwriting a newer one.
  const requestToken = useRef(0);

  // The request object is rebuilt on every render, so compare by value.
  const key = useMemo(
    () => JSON.stringify({ ...request, offset: 0, generation }),
    [request, generation],
  );

  const fetchPage = useCallback(
    async (page: number, token: number) => {
      if (inFlight.current.has(page)) return;
      inFlight.current.add(page);
      try {
        const response = await api.queryFiles({
          ...request,
          offset: page * PAGE_SIZE,
          limit: PAGE_SIZE,
        });
        if (token !== requestToken.current) return;
        setTotal(response.total);
        setBytes(response.bytes);
        setPages((prev) => {
          const next = new Map(prev);
          next.set(page, response.rows);
          return next;
        });
        setError(null);
      } catch (err) {
        if (token !== requestToken.current) return;
        const ipc = toIpcError(err);
        // "No scan yet" is the normal starting state, not a failure to report.
        setError(ipc.code === "noScan" ? null : ipc);
        if (ipc.code === "noScan") {
          setTotal(0);
          setBytes(0);
        }
      } finally {
        inFlight.current.delete(page);
        if (token === requestToken.current) setLoading(false);
      }
    },
    [request],
  );

  // Filter or scan changed: drop every cached page and reload the first one.
  useEffect(() => {
    requestToken.current += 1;
    const token = requestToken.current;
    inFlight.current.clear();
    setPages(new Map());
    setLoading(true);
    void fetchPage(0, token);
    // `key` is the value-identity of the request; fetchPage closes over it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const ensureRange = useCallback(
    (start: number, end: number) => {
      const firstPage = Math.floor(start / PAGE_SIZE);
      const lastPage = Math.floor(end / PAGE_SIZE);
      for (let page = firstPage; page <= lastPage; page++) {
        if (!pages.has(page)) void fetchPage(page, requestToken.current);
      }
    },
    [pages, fetchPage],
  );

  const getRow = useCallback(
    (index: number): FileRow | undefined => {
      const page = pages.get(Math.floor(index / PAGE_SIZE));
      return page?.[index % PAGE_SIZE];
    },
    [pages],
  );

  return { total, bytes, loading, error, getRow, ensureRange };
}
