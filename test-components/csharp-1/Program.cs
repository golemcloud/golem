using System.Collections;

var rand = new Random();
var now = DateTime.Now;

Console.WriteLine("Hello, World!");
Console.WriteLine(rand.NextInt64(0, 1000));
Console.WriteLine(now.Year);
Console.WriteLine(String.Join(", ", System.Environment.GetCommandLineArgs())); // NOTE: command line argument access does not work currently
foreach (DictionaryEntry kv in System.Environment.GetEnvironmentVariables()) {
    Console.WriteLine(kv.Key + ": " + kv.Value);
}
