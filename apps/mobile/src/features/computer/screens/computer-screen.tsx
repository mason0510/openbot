import { router, Stack, useLocalSearchParams } from "expo-router";
import { Button, Tabs, Typography } from "heroui-native";
import { useCallback, useEffect, useState } from "react";
import { View } from "react-native";
import { ComputerPreviewCard } from "@/features/computer/components/computer-preview-card";
import { useMobileWorkspace, useMobileWorkspaceActions } from "@/features/workspace/context/workspace-store";
import { showsOnline } from "@/features/workspace/model/server-status";
import { SheetScrollView } from "@/shared/components/sheet-scroll-view";
import { haptics } from "@/shared/lib/haptics";
import { useText } from "@/shared/lib/text";

type ComputerTab = "details" | "library" | "computer";

/**
 * Grok-style Computer surface: header tabs (详情 / 资料库 / 电脑) and a preview card with Open.
 * Open pushes the fullscreen live remote desktop route.
 */
export function ComputerScreen() {
  const { serverId, agentId } = useLocalSearchParams<{ serverId: string; agentId?: string }>();
  const { t } = useText();
  const { servers, agents, remoteDesktopSupport } = useMobileWorkspace();
  const { loadRemoteDesktopCapabilities } = useMobileWorkspaceActions();
  const server = servers.find((candidate) => candidate.id === serverId) ?? null;
  const agent = agents.find((candidate) => candidate.id === agentId && candidate.serverId === serverId) ?? null;
  const computerName = server?.name?.trim() || agent?.name || "Computer";
  const available = remoteDesktopSupport(serverId).available;
  const online = Boolean(server && showsOnline(server));
  const [tab, setTab] = useState<ComputerTab>("computer");
  const [previewUri, setPreviewUri] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!serverId || !available) return;
    setLoading(true);
    setError(null);
    try {
      const capabilities = await loadRemoteDesktopCapabilities(serverId);
      setReady(capabilities.ready);
      // A still thumbnail is optional; live view starts on Open. Keep null when the host has no preview API.
      setPreviewUri(null);
    } catch (cause) {
      setReady(false);
      setError(
        cause instanceof Error && cause.message === "unsupported"
          ? t("mobile.computer.status.unsupported")
          : t("mobile.computer.status.failed"),
      );
    } finally {
      setLoading(false);
    }
  }, [available, loadRemoteDesktopCapabilities, serverId, t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const openLive = () => {
    if (!serverId || !available || !online) return;
    void haptics.impact("soft");
    router.push({
      pathname: "/computer/[serverId]/live",
      params: { serverId, ...(agentId ? { agentId } : {}) },
    });
  };

  const openDetails = () => {
    if (!agentId) return;
    void haptics.impact("soft");
    router.push({ pathname: "/agent-info/[agentId]", params: { agentId, serverId } });
  };

  return (
    <>
      <Stack.Screen options={{ title: t("mobile.computer.header") }} />
      <SheetScrollView className="bg-sheet" contentContainerClassName="gap-5 pb-safe-offset-5 pt-2">
        <View className="px-5">
          <Tabs
            value={tab}
            onValueChange={(next) => {
              const value: ComputerTab =
                next === "details" || next === "library" || next === "computer" ? next : "computer";
              void haptics.selection();
              setTab(value);
              if (value === "details") openDetails();
            }}
          >
            <Tabs.List className="self-stretch rounded-grouped">
              <Tabs.Indicator className="rounded-[13px]" />
              <Tabs.Trigger value="details" className="flex-1 px-1">
                <Tabs.Label numberOfLines={1} className="text-sm">
                  {t("mobile.computer.detailsTab")}
                </Tabs.Label>
              </Tabs.Trigger>
              <Tabs.Trigger value="library" className="flex-1 px-1">
                <Tabs.Label numberOfLines={1} className="text-sm">
                  {t("mobile.computer.libraryTab")}
                </Tabs.Label>
              </Tabs.Trigger>
              <Tabs.Trigger value="computer" className="flex-1 px-1">
                <Tabs.Label numberOfLines={1} className="text-sm">
                  {t("mobile.computer.computerTab")}
                </Tabs.Label>
              </Tabs.Trigger>
            </Tabs.List>
          </Tabs>
        </View>

        {tab === "library" ? (
          <View className="gap-3 px-5">
            <Typography.Paragraph className="text-muted">{t("mobile.agent.info.files.title")}</Typography.Paragraph>
            {agentId ? (
              <Button
                variant="secondary"
                onPress={() =>
                  router.push({
                    pathname: "/agent-info/[agentId]/files",
                    params: { agentId, serverId },
                  })
                }
              >
                {t("mobile.agent.info.files.title")}
              </Button>
            ) : null}
          </View>
        ) : null}

        {tab === "computer" ? (
          <View className="gap-4">
            {!available ? (
              <Typography.Paragraph className="px-5 text-muted">
                {t("mobile.computer.status.unsupported")}
              </Typography.Paragraph>
            ) : !online ? (
              <Typography.Paragraph className="px-5 text-muted">
                {t("mobile.computer.status.offline")}
              </Typography.Paragraph>
            ) : error ? (
              <View className="gap-3 px-5">
                <Typography.Paragraph>{error}</Typography.Paragraph>
                <Button variant="secondary" onPress={() => void refresh()}>
                  {t("mobile.computer.retry")}
                </Button>
              </View>
            ) : !ready && !loading ? (
              <View className="gap-3 px-5">
                <Typography.Paragraph className="font-semibold">
                  {t("mobile.computer.status.notSharing", { name: computerName })}
                </Typography.Paragraph>
                <Typography.Paragraph className="text-muted">
                  {t("mobile.computer.status.notSharingBody")}
                </Typography.Paragraph>
                <Button variant="secondary" onPress={() => void refresh()}>
                  {t("mobile.computer.retry")}
                </Button>
              </View>
            ) : (
              <ComputerPreviewCard
                computerName={computerName}
                previewUri={previewUri}
                loading={loading}
                disabled={!online || !available}
                onOpen={openLive}
              />
            )}
          </View>
        ) : null}
      </SheetScrollView>
    </>
  );
}
