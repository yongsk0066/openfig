import { Platform } from "@fig/api-bindings";

export type PlatformInfo = Awaited<ReturnType<typeof Platform.getPlatformInfo>>;

export type PlatformRestrictions = Partial<PlatformInfo>;

export type PrefDefault = unknown;

export type Pref = {
  id: string;
  title: string;
  description?: string;
  example?: React.ReactNode;
  type: string;
  inverted?: boolean;
  default: PrefDefault;
  popular?: boolean;
  options?: string[];
  icon?: string;
  background?: {
    color: string;
    image: string;
  };
  priority?: number;
};

export type Action = {
  id: string;
  title: string;
  description?: string;
  availability: string;
  type: string;
  default: string[];
  popular?: boolean;
};
