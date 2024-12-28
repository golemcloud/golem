import { Monitor, Moon, Sun } from 'lucide-react';
import { useEffect, useState } from 'react';

export default function ThemeSelector() {
    const [theme, setTheme] = useState(() => {
        // Check for saved theme or system preference
        if (typeof window !== 'undefined') {
            return localStorage.getItem('theme') || 'system';
        }
        return 'system';
    });

    useEffect(() => {
        const root = window.document.documentElement;
        root.removeAttribute('data-theme');

        if (theme === 'system') {
            const systemTheme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
            root.setAttribute('data-theme', systemTheme);
        } else {
            root.setAttribute('data-theme', theme);
        }

        localStorage.setItem('theme', theme);
    }, [theme]);

    return (
        <div className="relative">
            <div className="flex items-center space-x-2 bg-card p-2 rounded-lg">
                <button
                    onClick={() => setTheme('light')}
                    className={`p-2 rounded-md ${theme === 'light' ? 'bg-primary text-white' : 'text-muted-foreground hover:text-white hover:bg-card/50'
                        }`}
                    title="Light mode"
                >
                    <Sun size={16} />
                </button>
                <button
                    onClick={() => setTheme('dark')}
                    className={`p-2 rounded-md ${theme === 'dark' ? 'bg-primary text-white' : 'text-muted-foreground hover:text-white hover:bg-card/50'
                        }`}
                    title="Dark mode"
                >
                    <Moon size={16} />
                </button>
                <button
                    onClick={() => setTheme('black')}
                    className={`p-2 rounded-md ${theme === 'black' ? 'bg-primary text-white' : 'text-muted-foreground hover:text-white hover:bg-card/50'
                        }`}
                    title="Black mode"
                >
                    <Moon size={16} className="fill-current" />
                </button>
                <button
                    onClick={() => setTheme('system')}
                    className={`p-2 rounded-md ${theme === 'system' ? 'bg-primary text-white' : 'text-muted-foreground hover:text-white hover:bg-card/50'
                        }`}
                    title="System preference"
                >
                    <Monitor size={16} />
                </button>
            </div>
        </div>
    );
}