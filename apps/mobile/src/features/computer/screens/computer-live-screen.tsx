import type { RemoteDesktopSession } from "@openbot/contracts/ipc";
import { GlassView } from "expo-glass-effect";
import { router, Stack, useLocalSearchParams } from "expo-router";
import { Button, Typography } from "heroui-native";
import X from "lucide-react-native/icons/x";
import { useCallback, useEffect, useRef, useState } from "react";
import { Pressable, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ComputerLiveToolbar } from "@/features/computer/components/computer-live-toolbar";
import type { RemoteDesktopViewerEvent } from "@/features/computer/model/remote-desktop-viewer-bridge";
import { useMobileWorkspace, useMobileWorkspaceActions } from "@/features/workspace/context/workspace-store";
import { showsOnline } from "@/features/workspace/model/server-status";
import { haptics } from "@/shared/lib/haptics";
import { useText } from "@/shared/lib/text";
import { useLiquidGlass } from "@/shared/lib/use-liquid-glass";

type LiveState = "connecting" | "connected" | "error";

/**
 * Fullscreen live remote desktop: Moonlight runs in the peer DOM overlay; this screen owns the
 * Grok-style chrome (close + bottom toolbar) and the session lifecycle.
 */
export function ComputerLiveScreen() {
  const { serverId } = useLocalSearchParams<{ serverId: string; agentId?: string }>();
  const { t, errorMessage } = useText();
  const insets = useSafeAreaInsets();
  const liquidGlass = useLiquidGlass();
  const { servers } = useMobileWorkspace();
  const { startRemoteDesktopSession, stopRemoteDesktopSession, openRemoteDesktopViewer } = useMobileWorkspaceActions();
  const server = servers.find((candidate) => candidate.id === serverId) ?? null;
  const online = Boolean(server && showsOnline(server));

  const [state, setState] = useState<LiveState>("connecting");
  const [message, setMessage] = useState<string | null>(null);
  const [cursorActive, setCursorActive] = useState(true);
  const keyboardOpenRef = useRef(false);
  const [zoom, setZoom] = useState(1);
  const sessionRef = useRef<RemoteDesktopSession | null>(null);
  const viewerRef = useRef<{ close: () => void; refresh: () => void } | null>(null);
  const keyboardRef = useRef<TextInput>(null);

  const cleanup = useCallback(async () => {
    viewerRef.current?.close();
    viewerRef.current = null;
    const session = sessionRef.current;
    sessionRef.current = null;
    if (session && serverId) {
      await stopRemoteDesktopSession(serverId, session.id).catch(() => undefined);
    }
  }, [serverId, stopRemoteDesktopSession]);

  const start = useCallback(async () => {
    if (!serverId || !online) {
      setState("error");
      setMessage(t("mobile.computer.status.offline"));
      return;
    }
    setState("connecting");
    setMessage(null);
    await cleanup();
    try {
      const session = await startRemoteDesktopSession(serverId);
      sessionRef.current = session;
      if (session.errorCode === "host_permissions_required") {
        setState("error");
        setMessage(t("mobile.computer.status.notSharingBody"));
        await stopRemoteDesktopSession(serverId, session.id).catch(() => undefined);
        sessionRef.current = null;
        return;
      }
      const handle = openRemoteDesktopViewer(serverId, session, (event: RemoteDesktopViewerEvent) => {
        if (event.type === "state") {
          setState(event.state);
          if (event.message) setMessage(event.message);
          if (event.state === "connected") setMessage(null);
        } else if (event.type === "ended") {
          setState("error");
          setMessage(event.reason ?? t("mobile.computer.status.failed"));
        }
      });
      if (!handle) {
        setState("error");
        setMessage(t("mobile.computer.status.failed"));
        await stopRemoteDesktopSession(serverId, session.id).catch(() => undefined);
        sessionRef.current = null;
        return;
      }
      viewerRef.current = handle;
    } catch (cause) {
      setState("error");
      setMessage(errorMessage(cause, t("mobile.computer.status.failed")));
    }
  }, [
    cleanup,
    errorMessage,
    online,
    openRemoteDesktopViewer,
    serverId,
    startRemoteDesktopSession,
    stopRemoteDesktopSession,
    t,
  ]);

  useEffect(() => {
    void start();
    return () => {
      void cleanup();
    };
  }, [cleanup, start]);

  const close = () => {
    void haptics.impact("soft");
    void cleanup().finally(() => {
      if (router.canGoBack()) router.back();
      else router.replace("/chat");
    });
  };

  return (
    <View className="flex-1 bg-black">
      <Stack.Screen options={{ headerShown: false, animation: "fade", gestureEnabled: false }} />

      {/* Status overlay while connecting / on error. The peer DOM draws the live stream above the app root. */}
      {state !== "connected" ? (
        <View className="absolute inset-0 z-10 items-center justify-center bg-black px-8">
          <Typography.Paragraph className="text-center text-white">
            {state === "connecting"
              ? t("mobile.computer.status.connecting")
              : (message ?? t("mobile.computer.status.failed"))}
          </Typography.Paragraph>
          {state === "error" ? (
            <View className="mt-4 flex-row gap-3">
              <Button variant="secondary" onPress={() => void start()}>
                {t("mobile.computer.retry")}
              </Button>
              <Button variant="ghost" onPress={close}>
                {t("mobile.computer.disconnect")}
              </Button>
            </View>
          ) : null}
        </View>
      ) : null}

      <View
        pointerEvents="box-none"
        className="absolute inset-x-0 top-0 z-20 flex-row items-center px-4"
        style={{ paddingTop: insets.top + 8 }}
      >
        <GlassView
          glassEffectStyle={liquidGlass ? "regular" : "none"}
          style={{
            backgroundColor: liquidGlass ? "transparent" : "rgba(20,20,22,0.92)",
            borderRadius: 24,
          }}
        >
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t("mobile.computer.close")}
            onPress={close}
            className="size-12 items-center justify-center"
          >
            <X color="#f5f5f5" size={22} strokeWidth={2} />
          </Pressable>
        </GlassView>
      </View>

      <ComputerLiveToolbar
        cursorActive={cursorActive}
        onRefresh={() => {
          viewerRef.current?.refresh();
          setState("connecting");
        }}
        onKeyboard={() => {
          const next = !keyboardOpenRef.current;
          keyboardOpenRef.current = next;
          requestAnimationFrame(() => (next ? keyboardRef.current?.focus() : keyboardRef.current?.blur()));
        }}
        onCursor={() => setCursorActive((value) => !value)}
        onMagnify={() => setZoom((value) => (value >= 2 ? 1 : value + 0.5))}
        onSettings={() => {
          void haptics.impact("soft");
          // Settings stay on the host for Screen Recording permissions messaging.
          setMessage(t("mobile.computer.status.notSharingBody"));
        }}
      />

      {/* Hidden field so the phone keyboard can type into the remote session later. */}
      <TextInput
        ref={keyboardRef}
        value=""
        onChangeText={() => undefined}
        style={{ position: "absolute", opacity: 0, height: 1, width: 1 }}
        autoCorrect={false}
        caretHidden
      />
      {zoom !== 1 && state === "connected" ? (
        <View className="absolute right-4 top-24 rounded-full bg-black/60 px-3 py-1">
          <Typography.Label className="text-white">{Math.round(zoom * 100)}%</Typography.Label>
        </View>
      ) : null}
    </View>
  );
}
