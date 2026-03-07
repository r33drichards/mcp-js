import type {
  OAuthClientProvider,
} from "npm:@modelcontextprotocol/sdk@1.12.1/client/auth.js";
import type {
  OAuthClientMetadata,
  OAuthClientInformation,
  OAuthClientInformationFull,
  OAuthTokens,
} from "npm:@modelcontextprotocol/sdk@1.12.1/shared/auth.js";

export interface ClientCredentialsConfig {
  tokenUrl: string;
  clientId: string;
  clientSecret: string;
  refreshBufferSecs?: number;
}

export class ClientCredentialsProvider implements OAuthClientProvider {
  private config: ClientCredentialsConfig;
  private cachedTokens?: OAuthTokens;
  private fetchedAt?: number;
  private refreshBufferSecs: number;

  constructor(config: ClientCredentialsConfig) {
    this.config = config;
    this.refreshBufferSecs = config.refreshBufferSecs ?? 30;
  }

  get redirectUrl(): string {
    return "http://localhost";
  }

  get clientMetadata(): OAuthClientMetadata {
    return {
      redirect_uris: ["http://localhost"],
      grant_types: ["client_credentials"],
      client_name: "mcp-client",
    };
  }

  clientInformation(): OAuthClientInformation {
    return {
      client_id: this.config.clientId,
      client_secret: this.config.clientSecret,
    };
  }

  saveClientInformation(_info: OAuthClientInformationFull): void {
    // No-op: pre-registered client
  }

  async tokens(): Promise<OAuthTokens | undefined> {
    if (this.cachedTokens && !this.isExpired()) {
      return this.cachedTokens;
    }
    await this.fetchToken();
    return this.cachedTokens;
  }

  saveTokens(tokens: OAuthTokens): void {
    this.cachedTokens = tokens;
    this.fetchedAt = Date.now();
  }

  redirectToAuthorization(_url: URL): void {
    throw new Error(
      "client_credentials grant does not support interactive authorization",
    );
  }

  saveCodeVerifier(_verifier: string): void {
    // No-op
  }

  codeVerifier(): string {
    return "";
  }

  private isExpired(): boolean {
    if (!this.cachedTokens || !this.fetchedAt) return true;

    let expiresIn = this.cachedTokens.expires_in;

    if (expiresIn === undefined) {
      // Decode JWT to get exp claim
      try {
        const parts = this.cachedTokens.access_token.split(".");
        const payload = JSON.parse(atob(parts[1]));
        if (payload.exp) {
          return Date.now() >= payload.exp * 1000 - this.refreshBufferSecs * 1000;
        }
      } catch {
        // Can't determine expiry, refresh to be safe
        return true;
      }
    }

    if (expiresIn !== undefined) {
      const expiresAt = this.fetchedAt + (expiresIn - this.refreshBufferSecs) * 1000;
      return Date.now() >= expiresAt;
    }

    return true;
  }

  private async fetchToken(): Promise<void> {
    const params: Record<string, string> = {
      grant_type: "client_credentials",
      client_id: this.config.clientId,
      client_secret: this.config.clientSecret,
    };

    const body = new URLSearchParams(params);
    const resp = await fetch(this.config.tokenUrl, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: body.toString(),
    });

    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(`Token request failed (${resp.status}): ${text}`);
    }

    const data = await resp.json();
    const tokens: OAuthTokens = {
      access_token: data.access_token,
      token_type: data.token_type,
      expires_in: data.expires_in,
      scope: data.scope,
      refresh_token: data.refresh_token,
    };

    this.cachedTokens = tokens;
    this.fetchedAt = Date.now();
  }
}
