// jsdom has no EventSource; this one records instances and lets tests push events.
export class FakeEventSource extends EventTarget {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  static instances: FakeEventSource[] = [];

  readonly url: string;
  readyState: number = FakeEventSource.CONNECTING;

  constructor(url: string | URL) {
    super();
    this.url = String(url);
    FakeEventSource.instances.push(this);
  }

  static latest(): FakeEventSource {
    const source = FakeEventSource.instances.at(-1);
    if (!source) throw new Error('no EventSource was opened');
    return source;
  }

  close(): void {
    this.readyState = FakeEventSource.CLOSED;
  }

  open(): void {
    this.readyState = FakeEventSource.OPEN;
    this.dispatchEvent(new Event('open'));
  }

  emit(type: string, data: unknown, id = ''): void {
    this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data), lastEventId: id }));
  }

  fail(): void {
    this.dispatchEvent(new Event('error'));
  }
}
