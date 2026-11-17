/** Effects supplied by a composition adapter, never acquired by completion policy. */
export interface Timing {
  now(): number;
  defer(callback: () => void): void;
  schedule(callback: () => void, milliseconds: number): () => void;
  delay(milliseconds: number, signal?: AbortSignal): Promise<void>;
  timeout<T>(milliseconds: number, operation: Promise<T>, signal?: AbortSignal): Promise<T>;
}
