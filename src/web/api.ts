type ApiErrorBody = { error?: { message?: string; code?: string } };

export class ApiError extends Error {
  status: number;
  code: string;

  constructor(message: string, status: number, code = 'API_ERROR') {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export async function api<T>(url: string, options: RequestInit = {}, csrfToken = ''): Promise<T> {
  const headers = new Headers(options.headers);
  if (options.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  if (csrfToken && options.method && options.method !== 'GET') headers.set('x-csrf-token', csrfToken);

  const response = await fetch(url, { ...options, headers, credentials: 'same-origin' });
  const body = response.status === 204 ? null : await response.json().catch(() => null) as ApiErrorBody | null;
  if (!response.ok) {
    throw new ApiError(body?.error?.message || 'İşlem tamamlanamadı.', response.status, body?.error?.code);
  }
  return body as T;
}
