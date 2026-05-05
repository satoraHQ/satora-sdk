export type LogLevel = "trace" | "debug" | "info" | "warn" | "error" | "silent";

export type ActiveLogLevel = Exclude<LogLevel, "silent">;

export interface LogRecord {
  level: ActiveLogLevel;
  message: string;
  event?: string;
  module?: string;
  operation?: string;
  swapId?: string;
  data?: Record<string, unknown>;
  error?: unknown;
}

export interface Logger {
  trace?(record: LogRecord): void;
  debug?(record: LogRecord): void;
  info?(record: LogRecord): void;
  warn?(record: LogRecord): void;
  error?(record: LogRecord): void;
}

export interface LoggerOptions {
  logger?: Logger;
  logLevel?: LogLevel;
}

export interface LogContext {
  module?: string;
  operation?: string;
  swapId?: string;
  data?: Record<string, unknown>;
}

const LEVEL_VALUE: Record<ActiveLogLevel, number> = {
  trace: 10,
  debug: 20,
  info: 30,
  warn: 40,
  error: 50,
};

const SECRET_FIELD_PATTERN = new RegExp(
  "(^|_|-|\\b)" +
    "(secret|secretkey|usersecretkey|privatekey|mnemonic|xprv|" +
    "preimage|signature|authorization|apikey|api_key|bearer)" +
    "(_|-|\\b|$)",
  "i",
);

const REDACTED = "[REDACTED]";

export const noopLogger: Logger = Object.freeze({});

function shouldLog(level: ActiveLogLevel, configuredLevel: LogLevel): boolean {
  if (configuredLevel === "silent") return false;
  return LEVEL_VALUE[level] >= LEVEL_VALUE[configuredLevel];
}

function serializeError(error: unknown): unknown {
  if (error instanceof Error) {
    return {
      name: error.name,
      message: error.message,
      stack: error.stack,
    };
  }
  return error;
}

export function redactLogValue(value: unknown): unknown {
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Error) return serializeError(value);
  if (Array.isArray(value)) return value.map(redactLogValue);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, nested] of Object.entries(
      value as Record<string, unknown>,
    )) {
      out[key] = SECRET_FIELD_PATTERN.test(key)
        ? REDACTED
        : redactLogValue(nested);
    }
    return out;
  }
  return value;
}

export class SdkLogger {
  readonly #logger: Logger;
  readonly #level: LogLevel;
  readonly #context: LogContext;

  constructor(options: LoggerOptions = {}, context: LogContext = {}) {
    this.#logger = options.logger ?? noopLogger;
    this.#level = options.logLevel ?? "silent";
    this.#context = context;
  }

  child(context: LogContext): SdkLogger {
    return new SdkLogger(
      { logger: this.#logger, logLevel: this.#level },
      {
        ...this.#context,
        ...context,
        data: {
          ...this.#context.data,
          ...context.data,
        },
      },
    );
  }

  trace(record: Omit<LogRecord, "level">): void {
    this.#emit("trace", record);
  }

  debug(record: Omit<LogRecord, "level">): void {
    this.#emit("debug", record);
  }

  info(record: Omit<LogRecord, "level">): void {
    this.#emit("info", record);
  }

  warn(record: Omit<LogRecord, "level">): void {
    this.#emit("warn", record);
  }

  error(record: Omit<LogRecord, "level">): void {
    this.#emit("error", record);
  }

  #emit(level: ActiveLogLevel, record: Omit<LogRecord, "level">): void {
    if (!shouldLog(level, this.#level)) return;

    const sink = this.#logger[level];
    if (!sink) return;

    const data = redactLogValue({
      ...this.#context.data,
      ...record.data,
    }) as Record<string, unknown>;

    sink({
      level,
      module: record.module ?? this.#context.module,
      operation: record.operation ?? this.#context.operation,
      swapId: record.swapId ?? this.#context.swapId,
      event: record.event,
      message: record.message,
      data: Object.keys(data).length > 0 ? data : undefined,
      error:
        record.error === undefined ? undefined : redactLogValue(record.error),
    });
  }
}

export function createSdkLogger(options?: LoggerOptions): SdkLogger {
  return new SdkLogger(options);
}

export function createConsoleLogger(): Logger {
  const write = (record: LogRecord) => {
    const { level, message, ...rest } = record;
    const payload = Object.fromEntries(
      Object.entries(rest).filter(([, value]) => value !== undefined),
    );
    const args =
      Object.keys(payload).length > 0 ? [message, payload] : [message];
    if (level === "trace") console.debug(...args);
    else if (level === "debug") console.debug(...args);
    else if (level === "info") console.info(...args);
    else if (level === "warn") console.warn(...args);
    else console.error(...args);
  };

  return {
    trace: write,
    debug: write,
    info: write,
    warn: write,
    error: write,
  };
}
