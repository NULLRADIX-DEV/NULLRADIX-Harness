import { Component, type ErrorInfo, type ReactNode } from 'react';
import { reportThemeFailure } from '../lib/themeFailure';

interface Props {
  // Shown instead of the theme component once that fails.
  fallback: ReactNode;
  // The theme id and what failed; a new theme tries its component again.
  themeId: string;
  where: string;
  children: ReactNode;
}

interface State {
  failed: boolean;
  themeId: string;
}

// A broken theme component never breaks the page: the built-in one takes its place.
export class SlotBoundary extends Component<Props, State> {
  override state: State = { failed: false, themeId: this.props.themeId };

  static getDerivedStateFromError(): Partial<State> {
    return { failed: true };
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    return props.themeId === state.themeId ? null : { failed: false, themeId: props.themeId };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    reportThemeFailure(this.props.themeId, this.props.where, error, info.componentStack);
  }

  override render(): ReactNode {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}
