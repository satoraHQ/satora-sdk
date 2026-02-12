/**
 * WebSocket price feed service for real-time price updates.
 *
 * This module provides a WebSocket client that connects to the Lendaswap
 * price feed endpoint and delivers real-time price updates with automatic
 * reconnection support.
 *
 * @example
 * ```typescript
 * import { PriceFeedService, type PriceUpdateMessage } from '@lendasat/lendaswap-sdk';
 *
 * const priceFeed = new PriceFeedService('wss://api.lendaswap.com');
 *
 * const unsubscribe = priceFeed.subscribe((update) => {
 *   console.log('Price update:', update);
 * });
 *
 * // Later, to unsubscribe:
 * unsubscribe();
 * ```
 */

import type { TokenId } from "./api/client.js";

/**
 * Price tiers for different quote asset amounts.
 * Different rates apply based on swap volume (in units of the quote asset).
 */
export interface PriceTiers {
  /** Rate when swapping 1 unit of the quote asset */
  tier_1: number;
  /** Rate when swapping 100 units of the quote asset */
  tier_100: number;
  /** Rate when swapping 1,000 units of the quote asset */
  tier_1000: number;
  /** Rate when swapping 5,000 units of the quote asset */
  tier_5000: number;
}

/**
 * Trading pair prices with volume-based tiers.
 */
export interface TradingPairPrices {
  /** Trading pair identifier, e.g., "USDC_POL-BTC" or "USDT0_POL-BTC" */
  pair: string;
  source: TokenId;
  target: TokenId;
  /** Price tiers for this pair */
  tiers: PriceTiers;
}

/**
 * Price update message received from WebSocket.
 */
export interface PriceUpdateMessage {
  /** Unix timestamp of the update */
  timestamp: number;
  /** Array of trading pair prices */
  pairs: TradingPairPrices[];
}

/**
 * Callback type for price updates.
 */
export type PriceUpdateCallback = (update: PriceUpdateMessage) => void;

/**
 * WebSocket price feed service with automatic reconnection.
 *
 * Manages connection to the /ws/prices endpoint with exponential backoff
 * reconnection. Automatically connects when the first listener subscribes
 * and disconnects when the last listener unsubscribes.
 *
 * @example
 * ```typescript
 * const priceFeed = new PriceFeedService('wss://api.lendaswap.com');
 *
 * // Subscribe to price updates
 * const unsubscribe = priceFeed.subscribe((update) => {
 *   console.log('Prices:', update.pairs);
 * });
 *
 * // Unsubscribe when done
 * unsubscribe();
 * ```
 */
export class PriceFeedService {
  private wsUrl: string;
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private listeners: Set<PriceUpdateCallback> = new Set();
  private reconnectDelay = 1000; // Start with 1 second
  private maxReconnectDelay = 30000; // Max 30 seconds
  private isManualClose = false;

  /**
   * Create a new PriceFeedService.
   *
   * @param baseUrl - The base WebSocket URL (e.g., 'wss://api.lendaswap.com')
   *                  or HTTP URL which will be converted to WebSocket.
   */
  constructor(baseUrl: string) {
    // Convert HTTP to WebSocket URL if needed
    this.wsUrl = baseUrl.replace(/^http/, "ws");
    // Ensure we have the /ws/prices path
    if (!this.wsUrl.endsWith("/ws/prices")) {
      this.wsUrl = `${this.wsUrl}/ws/prices`;
    }
  }

  /**
   * Subscribe to price updates.
   *
   * @param callback - Function to call when prices are updated
   * @returns Unsubscribe function
   */
  subscribe(callback: PriceUpdateCallback): () => void {
    this.listeners.add(callback);

    // Connect if not already connected
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      this.connect();
    }

    // Return unsubscribe function
    return () => {
      this.listeners.delete(callback);
      // Close connection if no more listeners
      if (this.listeners.size === 0) {
        this.close();
      }
    };
  }

  /**
   * Check if the WebSocket is currently connected.
   */
  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /**
   * Get the number of active listeners.
   */
  listenerCount(): number {
    return this.listeners.size;
  }

  /**
   * Connect to WebSocket price feed.
   */
  private connect(): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      return;
    }

    this.isManualClose = false;

    try {
      this.ws = new WebSocket(this.wsUrl);

      this.ws.onopen = () => {
        console.log("Price feed WebSocket connected");
        // Reset reconnect delay on successful connection
        this.reconnectDelay = 1000;
      };

      this.ws.onmessage = (event) => {
        try {
          const update: PriceUpdateMessage = JSON.parse(event.data);
          // Notify all listeners
          this.listeners.forEach((callback) => {
            try {
              callback(update);
            } catch (err) {
              console.error("Error in price feed callback:", err);
            }
          });
        } catch (error) {
          console.error("Failed to parse price update:", error);
        }
      };

      this.ws.onerror = (error) => {
        console.error("Price feed WebSocket error:", error);
      };

      this.ws.onclose = () => {
        console.log("Price feed WebSocket closed");
        this.ws = null;

        // Only reconnect if we have listeners and it wasn't a manual close
        if (this.listeners.size > 0 && !this.isManualClose) {
          this.scheduleReconnect();
        }
      };
    } catch (error) {
      console.error("Failed to create WebSocket:", error);
      if (this.listeners.size > 0 && !this.isManualClose) {
        this.scheduleReconnect();
      }
    }
  }

  /**
   * Schedule reconnection with exponential backoff.
   */
  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) {
      return;
    }

    console.log(`Reconnecting in ${this.reconnectDelay}ms...`);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
      // Exponential backoff
      this.reconnectDelay = Math.min(
        this.reconnectDelay * 2,
        this.maxReconnectDelay,
      );
    }, this.reconnectDelay);
  }

  /**
   * Close WebSocket connection.
   */
  close(): void {
    this.isManualClose = true;

    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }
}
