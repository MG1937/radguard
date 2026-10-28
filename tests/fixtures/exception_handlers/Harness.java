package exceptions;

import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Executes generated Java against the instruction/handler rules in the Smali. */
public class Harness {
    private interface Action { Object run() throws Throwable; }

    private static String outcome(Action action) {
        Support.events.clear();
        try {
            return "return:" + action.run() + ":" + Support.events;
        } catch (Throwable thrown) {
            return "throw:" + thrown.getClass().getName() + ":" + Support.events;
        }
    }

    // A small instruction-level model: only the listed instruction sites are
    // protected. Handler calls and normal continuations are outside that range.
    private static Object expected(String method) throws Throwable {
        if (List.of("two", "sameType", "three", "nested").contains(method)) {
            String[] sites = method.equals("three")
                ? new String[] {"first", "second", "third", "outside"}
                : method.equals("nested")
                    ? new String[] {"first", "second", "outerTail"}
                    : new String[] {"first", "second", "outside"};
            for (String site : sites) {
                try {
                    Support.hit(site);
                } catch (Throwable thrown) {
                    String handler = null;
                    if (method.equals("nested")) {
                        if (site.equals("second") && thrown instanceof IllegalStateException)
                            return null;
                        if (thrown instanceof IllegalArgumentException)
                            handler = "caughtFirst";
                    } else if (site.equals("first") && thrown instanceof IllegalArgumentException) {
                        handler = "caughtFirst";
                    } else if (site.equals("second") && (method.equals("sameType")
                            ? thrown instanceof IllegalArgumentException
                            : thrown instanceof IllegalStateException)) {
                        handler = "caughtSecond";
                    } else if (site.equals("third") && thrown instanceof UnsupportedOperationException) {
                        handler = "caughtThird";
                    }
                    if (handler == null) throw thrown;
                    Support.hit(handler);
                    return null;
                }
            }
            return null;
        }
        try {
            Support.hit("fail");
        } catch (Throwable thrown) {
            if (method.equals("catchAllOnly")) return 1;
            if (method.equals("typedAndCatchAll"))
                return thrown instanceof IllegalArgumentException ? 1 : 2;
            if (method.equals("reuseForReference") && thrown instanceof IllegalArgumentException) {
                Support.hit("consume:caught");
                return null;
            }
            if (method.equals("reuseForInt")) {
                if (thrown instanceof IllegalArgumentException) return 1;
                if (thrown instanceof IllegalStateException) return 2;
            }
            if (method.equals("multiCatch") && (thrown instanceof IllegalArgumentException
                    || thrown instanceof IllegalStateException)) return 3;
            throw thrown;
        }
        return method.equals("reuseForReference") ? null : Integer.valueOf(0);
    }

    public static void main(String[] args) {
        Throwable[] failures = {
            new IllegalArgumentException(), new IllegalStateException(),
            new UnsupportedOperationException(), new IOException(), new AssertionError()
        };
        String[] sites = {"", "first", "second", "third", "outerTail", "outside", "fail"};
        String[] handlers = {"", "caughtFirst", "caughtSecond", "caughtThird", "consume:caught"};
        int count = 0;
        for (Class<?> cls : new Class<?>[] {AdjacentTry.class, CatchTypes.class}) {
            for (Method method : cls.getDeclaredMethods()) {
                if (args.length > 0 && !method.getName().equals(args[0])) continue;
                for (String site : sites) {
                    for (Throwable failure : failures) {
                        for (String handler : handlers) {
                            for (Throwable handlerFailure : failures) {
                                Support.failures.clear();
                                Support.failures.put(site, failure);
                                Support.failures.put(handler, handlerFailure);
                                String wanted = outcome(() -> expected(method.getName()));
                                String actual = outcome(() -> {
                                    try { return method.invoke(null); }
                                    catch (InvocationTargetException wrapped) { throw wrapped.getCause(); }
                                });
                                if (!wanted.equals(actual)) {
                                    throw new AssertionError(method.getName() + " at " + site
                                        + " with " + failure.getClass().getName() + " / " + handler
                                        + ": expected " + wanted + ", got " + actual);
                                }
                                count++;
                            }
                        }
                    }
                }
            }
        }
        System.out.println("PASS " + count + " generated-Java behavior cases");
    }
}

class Support {
    static final List<String> events = new ArrayList<>();
    static final Map<String, Throwable> failures = new HashMap<>();

    static void hit(String site) {
        events.add(site);
        Throwable failure = failures.get(site);
        if (failure != null) Support.<RuntimeException>raise(failure);
    }

    @SuppressWarnings("unchecked")
    private static <T extends Throwable> void raise(Throwable failure) throws T {
        throw (T) failure;
    }

    static void first() { hit("first"); }
    static void second() { hit("second"); }
    static void third() { hit("third"); }
    static void outside() { hit("outside"); }
    static void outerTail() { hit("outerTail"); }
    static void caughtFirst() { hit("caughtFirst"); }
    static void caughtSecond() { hit("caughtSecond"); }
    static void caughtThird() { hit("caughtThird"); }
    static void fail() { hit("fail"); }
    static void consume(String value) { hit("consume:" + value); }
}
