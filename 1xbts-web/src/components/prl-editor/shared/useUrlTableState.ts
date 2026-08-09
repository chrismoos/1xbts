import { Dispatch, SetStateAction, useEffect, useRef, useState } from "react";
import { SortDirection } from "./TableFilters";

function useUrlState<T>(
  key: string,
  fallback: T,
  parse: (raw: string | null, fallback: T) => T,
  serialize: (value: T) => string,
): [T, Dispatch<SetStateAction<T>>] {
  const parseRef = useRef(parse);
  const serializeRef = useRef(serialize);
  const fallbackRef = useRef(fallback);

  useEffect(() => {
    parseRef.current = parse;
    serializeRef.current = serialize;
    fallbackRef.current = fallback;
  }, [fallback, parse, serialize]);

  const [value, setValue] = useState<T>(() => {
    if (typeof window === "undefined") return fallback;
    return parse(new URL(window.location.href).searchParams.get(key), fallback);
  });

  useEffect(() => {
    const restore = () => {
      const raw = new URL(window.location.href).searchParams.get(key);
      setValue(parseRef.current(raw, fallbackRef.current));
    };
    window.addEventListener("popstate", restore);
    window.addEventListener("hashchange", restore);
    return () => {
      window.removeEventListener("popstate", restore);
      window.removeEventListener("hashchange", restore);
    };
  }, [key]);

  useEffect(() => {
    const url = new URL(window.location.href);
    const encoded = serializeRef.current(value);
    const encodedFallback = serializeRef.current(fallbackRef.current);
    if (encoded === encodedFallback) url.searchParams.delete(key);
    else url.searchParams.set(key, encoded);
    if (url.href !== window.location.href) {
      window.history.replaceState(window.history.state, "", url);
    }
  }, [key, value]);

  return [value, setValue];
}

export function useUrlStringState(
  key: string,
  fallback = "",
): [string, Dispatch<SetStateAction<string>>] {
  return useUrlState(
    key,
    fallback,
    (raw, defaultValue) => raw ?? defaultValue,
    (value) => value,
  );
}

export function useUrlStringListState(
  key: string,
): [string[], Dispatch<SetStateAction<string[]>>] {
  return useUrlState(
    key,
    [],
    (raw) => (raw ? raw.split(",").filter(Boolean) : []),
    (value) => value.join(","),
  );
}

function numericRows(rows: string[]): number[] {
  return rows
    .map(Number)
    .filter((row) => Number.isInteger(row) && row >= 0);
}

function serializeRows(rows: number[]): string[] {
  return [...new Set(rows)].sort((left, right) => left - right).map(String);
}

export function toggleOpenRow(rows: string[], index: number): string[] {
  const current = numericRows(rows);
  return serializeRows(
    current.includes(index)
      ? current.filter((row) => row !== index)
      : [...current, index],
  );
}

export function remapOpenRowsAfterMove(
  rows: string[],
  from: number,
  to: number,
): string[] {
  return serializeRows(
    numericRows(rows).map((row) => {
      if (row === from) return to;
      if (from < to && row > from && row <= to) return row - 1;
      if (from > to && row >= to && row < from) return row + 1;
      return row;
    }),
  );
}

export function remapOpenRowsAfterRemove(
  rows: string[],
  removed: number,
): string[] {
  return serializeRows(
    numericRows(rows)
      .filter((row) => row !== removed)
      .map((row) => (row > removed ? row - 1 : row)),
  );
}

export function useUrlSortState<Key extends string>(
  key: string,
  validKeys: readonly Key[],
  fallback: { key: Key; direction: SortDirection },
): [
  { key: Key; direction: SortDirection },
  Dispatch<SetStateAction<{ key: Key; direction: SortDirection }>>,
] {
  return useUrlState(
    key,
    fallback,
    (raw, defaultValue) => {
      const [sortKey, direction] = raw?.split(":") ?? [];
      if (
        validKeys.includes(sortKey as Key) &&
        (direction === "asc" || direction === "desc")
      ) {
        return { key: sortKey as Key, direction };
      }
      return defaultValue;
    },
    (value) => `${value.key}:${value.direction}`,
  );
}
