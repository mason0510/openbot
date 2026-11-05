import { Typography } from "heroui-native";
import Monitor from "lucide-react-native/icons/monitor";
import { Image, Pressable, View } from "react-native";
import { haptics } from "@/shared/lib/haptics";
import { useText } from "@/shared/lib/text";

interface ComputerPreviewCardProps {
  computerName: string;
  previewUri?: string | null;
  loading?: boolean;
  disabled?: boolean;
  onOpen: () => void;
}

/** Grok-style Computer tab card: screen thumbnail, “xxx's screen”, and a black Open pill. */
export function ComputerPreviewCard({
  computerName,
  previewUri,
  loading = false,
  disabled = false,
  onOpen,
}: ComputerPreviewCardProps) {
  const { t } = useText();
  return (
    <View className="px-5">
      <View className="overflow-hidden rounded-3xl border border-separator bg-black">
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t("mobile.computer.openLive")}
          disabled={disabled || loading}
          onPress={() => {
            void haptics.impact("soft");
            onOpen();
          }}
          className="aspect-[4/3] items-center justify-center bg-[#0b0d0f]"
        >
          {previewUri ? (
            <Image
              source={{ uri: previewUri }}
              className="size-full"
              resizeMode="contain"
              accessibilityIgnoresInvertColors
            />
          ) : (
            <View className="items-center gap-3 px-6">
              <Monitor color="#8b929a" size={40} strokeWidth={1.75} />
              <Typography.Paragraph className="text-center text-muted" numberOfLines={2}>
                {loading ? t("mobile.computer.status.connecting") : t("mobile.computer.previewUnavailable")}
              </Typography.Paragraph>
            </View>
          )}
        </Pressable>
        <View className="flex-row items-center gap-3 border-t border-white/10 px-4 py-3">
          <Typography.Paragraph className="min-w-0 flex-1 text-white" numberOfLines={1}>
            {t("mobile.computer.screenOf", { name: computerName })}
          </Typography.Paragraph>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t("mobile.computer.openLive")}
            disabled={disabled || loading}
            onPress={() => {
              void haptics.impact("soft");
              onOpen();
            }}
            className="rounded-full px-4 py-2 active:opacity-80 disabled:opacity-40"
            style={{ backgroundColor: "#111" }}
          >
            <Typography.Label className="font-semibold text-white">{t("mobile.computer.open")}</Typography.Label>
          </Pressable>
        </View>
      </View>
    </View>
  );
}
