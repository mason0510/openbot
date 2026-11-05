import { GlassView } from "expo-glass-effect";
import Keyboard from "lucide-react-native/icons/keyboard";
import MousePointer2 from "lucide-react-native/icons/mouse-pointer-2";
import RefreshCw from "lucide-react-native/icons/refresh-cw";
import Settings from "lucide-react-native/icons/settings";
import ZoomIn from "lucide-react-native/icons/zoom-in";
import type { ReactNode } from "react";
import { Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { haptics } from "@/shared/lib/haptics";
import { useText } from "@/shared/lib/text";
import { useLiquidGlass } from "@/shared/lib/use-liquid-glass";

interface ComputerLiveToolbarProps {
  onRefresh: () => void;
  onKeyboard: () => void;
  onCursor: () => void;
  onMagnify: () => void;
  onSettings: () => void;
  cursorActive?: boolean;
}

function ToolButton({
  label,
  active,
  onPress,
  children,
}: {
  label: string;
  active?: boolean;
  onPress: () => void;
  children: ReactNode;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={() => {
        void haptics.impact("soft");
        onPress();
      }}
      className="size-12 items-center justify-center rounded-full"
      style={{ backgroundColor: active ? "rgba(255,255,255,0.18)" : "transparent" }}
    >
      {children}
    </Pressable>
  );
}

/** Bottom toolbar matching Grok remote desktop: refresh / keyboard / cursor / magnify / settings. */
export function ComputerLiveToolbar({
  onRefresh,
  onKeyboard,
  onCursor,
  onMagnify,
  onSettings,
  cursorActive = true,
}: ComputerLiveToolbarProps) {
  const { t } = useText();
  const insets = useSafeAreaInsets();
  const liquidGlass = useLiquidGlass();
  const icon = "#f5f5f5";
  return (
    <View
      pointerEvents="box-none"
      className="absolute inset-x-0 bottom-0 items-center"
      style={{ paddingBottom: Math.max(insets.bottom, 12) }}
    >
      <GlassView
        glassEffectStyle={liquidGlass ? "regular" : "none"}
        style={{
          backgroundColor: liquidGlass ? "transparent" : "rgba(20,20,22,0.92)",
          borderRadius: 28,
          flexDirection: "row",
          gap: 4,
          paddingHorizontal: 10,
          paddingVertical: 6,
        }}
      >
        <ToolButton label={t("mobile.computer.refresh")} onPress={onRefresh}>
          <RefreshCw color={icon} size={22} strokeWidth={2} />
        </ToolButton>
        <ToolButton label={t("mobile.computer.keyboard")} onPress={onKeyboard}>
          <Keyboard color={icon} size={22} strokeWidth={2} />
        </ToolButton>
        <ToolButton label={t("mobile.computer.cursor")} active={cursorActive} onPress={onCursor}>
          <MousePointer2 color={icon} size={22} strokeWidth={2} />
        </ToolButton>
        <ToolButton label={t("mobile.computer.magnify")} onPress={onMagnify}>
          <ZoomIn color={icon} size={22} strokeWidth={2} />
        </ToolButton>
        <ToolButton label={t("mobile.computer.settings")} onPress={onSettings}>
          <Settings color={icon} size={22} strokeWidth={2} />
        </ToolButton>
      </GlassView>
    </View>
  );
}
